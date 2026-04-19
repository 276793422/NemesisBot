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
	conn      *net.UDPConn
	port      int
	mu        sync.RWMutex
	running   bool
	stopCh    chan struct{}
	onMessage func(*DiscoveryMessage, *net.UDPAddr)
	encKey    []byte // AES encryption key (nil = no encryption)
}

// NewUDPListener creates a new UDP listener.
// encKey is the AES-256 key for broadcast encryption; pass nil to disable encryption.
func NewUDPListener(port int, encKey []byte) (*UDPListener, error) {
	// Listen on all interfaces (IPv4)
	addr, err := net.ResolveUDPAddr("udp4", fmt.Sprintf(":%d", port))
	if err != nil {
		return nil, fmt.Errorf("failed to resolve UDP address: %w", err)
	}

	conn, err := net.ListenUDP("udp", addr)
	if err != nil {
		return nil, fmt.Errorf("failed to listen on UDP port %d: %w", port, err)
	}

	// Capture the actual port (important when port 0 is used for auto-assignment)
	actualPort := conn.LocalAddr().(*net.UDPAddr).Port

	return &UDPListener{
		conn:   conn,
		port:   actualPort,
		stopCh: make(chan struct{}),
		encKey: encKey,
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
	buf := make([]byte, 4096)

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

			// Decrypt if encryption is enabled
			rawData := buf[:n]
			var msgData []byte
			if l.encKey != nil {
				decrypted, err := decryptData(l.encKey, rawData)
				if err != nil {
					// Decryption failed (non-cluster node or tampered data), silently discard
					continue
				}
				msgData = decrypted
			} else {
				msgData = rawData
			}

			// Parse message
			var msg DiscoveryMessage
			if err := json.Unmarshal(msgData, &msg); err != nil {
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

	// Encrypt if encryption is enabled
	if l.encKey != nil {
		encrypted, err := encryptData(l.encKey, data)
		if err != nil {
			return fmt.Errorf("failed to encrypt message: %w", err)
		}
		data = encrypted
	}

	// Get local broadcast addresses
	broadcastAddrs := l.getBroadcastAddresses()

	// Broadcast to all addresses on listener's port
	basePort := l.GetPort()
	for _, addr := range broadcastAddrs {
		targetAddr, err := net.ResolveUDPAddr("udp", fmt.Sprintf("%s:%d", addr, basePort))
		if err != nil {
			continue
		}
		l.conn.WriteToUDP(data, targetAddr)
	}

	return nil
}

// getBroadcastAddresses returns list of broadcast addresses to use
func (l *UDPListener) getBroadcastAddresses() []string {
	broadcastList := []string{
		"255.255.255.255", // Global broadcast
	}

	// Try to get local subnet broadcast addresses
	interfaces, err := net.Interfaces()
	if err != nil {
		return broadcastList
	}

	for _, iface := range interfaces {
		// Skip down interfaces and loopback
		if iface.Flags&net.FlagUp == 0 || iface.Flags&net.FlagLoopback != 0 {
			continue
		}

		ifaceAddrs, err := iface.Addrs()
		if err != nil {
			continue
		}

		for _, addr := range ifaceAddrs {
			ipNet, ok := addr.(*net.IPNet)
			if !ok || ipNet.IP == nil || ipNet.IP.IsLoopback() {
				continue
			}

			// Only handle IPv4
			ip4 := ipNet.IP.To4()
			if ip4 == nil {
				continue
			}

			// Calculate correct broadcast address using the subnet mask
			mask := ipNet.Mask
			if len(mask) != 4 {
				continue
			}

			// broadcast = ip | ^mask
			broadcastIP := make(net.IP, 4)
			for i := range ip4 {
				broadcastIP[i] = ip4[i] | ^mask[i]
			}

			broadcastList = append(broadcastList, broadcastIP.String())
		}
	}

	return broadcastList
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
