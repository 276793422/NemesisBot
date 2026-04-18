// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package discovery

import (
	"fmt"
	"math/rand"
	"net"
	"sync"
	"time"
)

// LogFunc is a logging function callback
type LogFunc func(format string, args ...interface{})

// ClusterCallbacks defines the interface that discovery uses to interact with the cluster
type ClusterCallbacks interface {
	GetNodeID() string
	GetAddress() string
	GetRPCPort() int
	GetAllLocalIPs() []string
	GetRole() string
	GetCategory() string
	GetTags() []string
	LogInfo(msg string, args ...interface{})
	LogError(msg string, args ...interface{})
	LogDebug(msg string, args ...interface{})
	HandleDiscoveredNode(nodeID, name string, addresses []string, rpcPort int, role, category string, tags []string, capabilities []string)
	HandleNodeOffline(nodeID, reason string)
	SyncToDisk() error
}

// Discovery handles UDP broadcast discovery
// Discovery manages UDP-based node discovery in the cluster.
//
// Security note: UDP broadcast has no authentication — any process on the LAN can send
// forged announce/bye messages. This is acceptable for the trusted-LAN design target.
// RPC connections (TCP) are protected by token authentication (see rpc/server.go SetAuthToken).
// If deploying in untrusted networks, add HMAC signing to DiscoveryMessage (shared secret from peers.toml).
// This is a known limitation, NOT a bug.
type Discovery struct {
	cluster           ClusterCallbacks
	listener          *UDPListener
	broadcastInterval time.Duration
	mu                sync.RWMutex
	running           bool
	stopCh            chan struct{}
}

// NewDiscovery creates a new discovery instance
func NewDiscovery(port int, cluster ClusterCallbacks) (*Discovery, error) {
	listener, err := NewUDPListener(port)
	if err != nil {
		return nil, err
	}

	return &Discovery{
		cluster:           cluster,
		listener:          listener,
		broadcastInterval: 30 * time.Second,
		stopCh:            make(chan struct{}),
	}, nil
}

// SetBroadcastInterval sets the broadcast interval
func (d *Discovery) SetBroadcastInterval(interval time.Duration) {
	d.mu.Lock()
	defer d.mu.Unlock()
	d.broadcastInterval = interval
}

// Start starts the discovery service
func (d *Discovery) Start() error {
	d.mu.Lock()
	if d.running {
		d.mu.Unlock()
		return fmt.Errorf("discovery already running")
	}
	d.running = true
	d.mu.Unlock()

	// Set message handler
	d.listener.SetMessageHandler(d.handleMessage)

	// Start listener
	if err := d.listener.Start(); err != nil {
		return fmt.Errorf("failed to start listener: %w", err)
	}

	d.cluster.LogInfo("Discovery started on port %d", d.listener.GetPort())

	// Send initial announce
	go d.sendAnnounce()

	// Start broadcast loop
	go d.broadcastLoop()

	return nil
}

// Stop stops the discovery service
func (d *Discovery) Stop() error {
	d.mu.Lock()
	if !d.running {
		d.mu.Unlock()
		return fmt.Errorf("discovery not running")
	}
	d.running = false

	// Signal stop
	close(d.stopCh)

	d.mu.Unlock()

	// Stop listener
	if err := d.listener.Stop(); err != nil {
		return fmt.Errorf("failed to stop listener: %w", err)
	}

	d.cluster.LogInfo("Discovery stopped")

	return nil
}

// broadcastLoop runs the broadcast loop
func (d *Discovery) broadcastLoop() {
	ticker := time.NewTicker(d.broadcastInterval)
	defer ticker.Stop()

	// Initial announce with jitter
	time.AfterFunc(jitter(5*time.Second), func() {
		d.sendAnnounce()
	})

	for {
		select {
		case <-ticker.C:
			d.sendAnnounce()

		case <-d.stopCh:
			return
		}
	}
}

// sendAnnounce sends an announce broadcast
func (d *Discovery) sendAnnounce() {
	// Get all local IPs
	addresses := d.cluster.GetAllLocalIPs()
	if len(addresses) == 0 {
		d.cluster.LogError("No local IP addresses available for broadcast")
		return
	}

	msg := NewAnnounceMessage(
		d.cluster.GetNodeID(),
		d.cluster.GetNodeID(), // Use nodeID as name for now, cluster can override
		addresses,
		d.cluster.GetRPCPort(),
		d.cluster.GetRole(),
		d.cluster.GetCategory(),
		d.cluster.GetTags(),
		[]string{}, // Capabilities will be set by cluster module
	)

	if err := d.listener.Broadcast(msg); err != nil {
		d.cluster.LogError("Failed to send announce: %v", err)
	} else {
		d.cluster.LogDebug("Announce sent: node_id=%s, addresses=%v, rpc_port=%d", d.cluster.GetNodeID(), addresses, d.cluster.GetRPCPort())
	}
}

// handleMessage handles a received discovery message
func (d *Discovery) handleMessage(msg *DiscoveryMessage, addr *net.UDPAddr) {
	// Ignore messages from self
	if msg.NodeID == d.cluster.GetNodeID() {
		return
	}

	// Ignore expired messages
	if msg.IsExpired() {
		d.cluster.LogDebug("Ignoring expired message from %s", msg.NodeID)
		return
	}

	d.cluster.LogInfo("Received %s from %s (%s)", msg.Type, msg.NodeID, addr.String())

	switch msg.Type {
	case MessageTypeAnnounce:
		d.handleAnnounce(msg)

	case MessageTypeBye:
		d.handleBye(msg)
	}
}

// handleAnnounce handles an announce message
func (d *Discovery) handleAnnounce(msg *DiscoveryMessage) {
	// Use callback to handle discovered node
	d.cluster.HandleDiscoveredNode(msg.NodeID, msg.Name, msg.Addresses, msg.RPCPort, msg.Role, msg.Category, msg.Tags, msg.Capabilities)

	d.cluster.LogInfo("Node discovered/updated: %s", msg.NodeID)

	// Sync to disk immediately on new discovery.
	// NOTE: This is synchronous and runs in the UDP receive loop. Currently acceptable because:
	// - state.toml is small (~KB), write takes <1ms
	// - Discovery broadcasts every 30s, so write frequency is low
	// - If node count exceeds 100+, consider debouncing: mark dirty here, let syncLoop write.
	// This is NOT a bug — do not change to async unless I/O latency becomes measurable.
	if err := d.cluster.SyncToDisk(); err != nil {
		d.cluster.LogError("Failed to sync config: %v", err)
	}
}

// handleBye handles a bye message
func (d *Discovery) handleBye(msg *DiscoveryMessage) {
	d.cluster.HandleNodeOffline(msg.NodeID, "node shutdown")

	d.cluster.LogInfo("Node marked offline: %s (bye)", msg.NodeID)

	// Sync to disk
	if err := d.cluster.SyncToDisk(); err != nil {
		d.cluster.LogError("Failed to sync config: %v", err)
	}
}

// IsRunning returns true if discovery is running
func (d *Discovery) IsRunning() bool {
	d.mu.RLock()
	defer d.mu.RUnlock()
	return d.running
}

// jitter returns a random duration between -maxJitter and +maxJitter
func jitter(maxJitter time.Duration) time.Duration {
	return time.Duration(rand.Int63n(int64(maxJitter)*2) - int64(maxJitter))
}
