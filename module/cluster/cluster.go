// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package cluster

import (
	"fmt"
	"os"
	"path/filepath"
	"sync"
	"time"

	"github.com/276793422/NemesisBot/module/cluster/discovery"
	"github.com/276793422/NemesisBot/module/cluster/rpc"
)

const (
	// DefaultUDPPort is the default UDP broadcast port
	DefaultUDPPort = 49100
	// DefaultRPCPort is the default WebSocket RPC port
	DefaultRPCPort = 49200
	// DefaultBroadcastInterval is the default broadcast interval
	DefaultBroadcastInterval = 30 * time.Second
	// DefaultTimeout is the default timeout for marking a node as offline
	DefaultTimeout = 90 * time.Second
)

// Cluster represents the bot cluster
type Cluster struct {
	// Node information
	nodeID   string
	nodeName string
	address  string

	// Paths
	workspace       string
	staticConfigPath string  // peers.toml (static configuration)
	dynamicStatePath string  // state.toml (dynamic state)
	logDir          string

	// Components
	registry  *Registry
	logger    *ClusterLogger
	discovery *discovery.Discovery
	rpcClient *rpc.Client

	// Configuration
	udpPort           int
	rpcPort           int
	broadcastInterval time.Duration
	timeout           time.Duration

	// State
	mu      sync.RWMutex
	running bool
	stopCh  chan struct{}
}

// NewCluster creates a new cluster instance
func NewCluster(workspace string) (*Cluster, error) {
	// Generate node ID
	nodeID, err := GenerateNodeID()
	if err != nil {
		return nil, fmt.Errorf("failed to generate node ID: %w", err)
	}

	// Create cluster directory
	clusterDir := filepath.Join(workspace, "cluster")
	if err := os.MkdirAll(clusterDir, 0755); err != nil {
		return nil, fmt.Errorf("failed to create cluster directory: %w", err)
	}

	// Config paths
	staticConfigPath := filepath.Join(clusterDir, "peers.toml")   // Static configuration
	dynamicStatePath := filepath.Join(clusterDir, "state.toml")   // Dynamic state

	// Create logger
	logger, err := NewClusterLogger(workspace)
	if err != nil {
		return nil, fmt.Errorf("failed to create logger: %w", err)
	}

	cluster := &Cluster{
		nodeID:           nodeID,
		nodeName:         "Bot " + nodeID,
		workspace:        workspace,
		staticConfigPath: staticConfigPath,
		dynamicStatePath: dynamicStatePath,
		registry:         NewRegistry(),
		logger:           logger,
		udpPort:          49100,  // Default UDP port
		rpcPort:          49200,  // Default RPC port
		broadcastInterval: DefaultBroadcastInterval,
		timeout:          DefaultTimeout,
		stopCh:           make(chan struct{}),
	}

	// Load static config if available (contains manually configured peers)
	if err := cluster.loadStaticConfig(); err != nil {
		logger.DiscoveryError("Failed to load static config: %v", err)
		// Continue anyway, will use defaults
	}

	// Load dynamic state if available (contains discovered peers)
	if err := cluster.loadDynamicState(); err != nil {
		logger.DiscoveryError("Failed to load dynamic state: %v", err)
		// Continue anyway, will start fresh
	}

	logger.DiscoveryInfo("Cluster initialized: node_id=%s", nodeID)

	return cluster, nil
}

// Start starts the cluster
func (c *Cluster) Start() error {
	c.mu.Lock()
	if c.running {
		c.mu.Unlock()
		return fmt.Errorf("cluster already running")
	}
	c.running = true
	c.mu.Unlock()

	// Get local IP for RPC address
	localIP, err := getLocalIP()
	if err != nil {
		return fmt.Errorf("failed to get local IP: %w", err)
	}
	c.address = fmt.Sprintf("%s:%d", localIP, c.rpcPort)

	// Initialize discovery
	disc, err := discovery.NewDiscovery(c.udpPort, c)
	if err != nil {
		return fmt.Errorf("failed to create discovery: %w", err)
	}
	c.discovery = disc

	// Initialize RPC client
	c.rpcClient = rpc.NewClient(c)

	// Start discovery
	if err := c.discovery.Start(); err != nil {
		return fmt.Errorf("failed to start discovery: %w", err)
	}

	// Start RPC server (will run in background)
	rpcServer := rpc.NewServer(c)
	if err := rpcServer.Start(c.rpcPort); err != nil {
		return fmt.Errorf("failed to start RPC server: %w", err)
	}

	c.logger.DiscoveryInfo("Cluster started: node_id=%s, udp_port=%d, rpc_port=%d, address=%s",
		c.nodeID, c.udpPort, c.rpcPort, c.address)

	// Start background tasks
	go c.syncLoop()

	return nil
}

// Stop stops the cluster
func (c *Cluster) Stop() error {
	c.mu.Lock()
	if !c.running {
		c.mu.Unlock()
		return fmt.Errorf("cluster not running")
	}
	c.running = false

	// Signal stop
	close(c.stopCh)

	c.mu.Unlock()

	// Stop discovery
	if c.discovery != nil {
		if err := c.discovery.Stop(); err != nil {
			c.logger.DiscoveryError("Failed to stop discovery: %v", err)
		}
	}

	// Close RPC client
	if c.rpcClient != nil {
		if err := c.rpcClient.Close(); err != nil {
			c.logger.RPCError("Failed to close RPC client: %v", err)
		}
	}

	c.logger.DiscoveryInfo("Cluster stopped: node_id=%s", c.nodeID)

	// Close logger
	return c.logger.Close()
}

// GetNodeID returns the node ID
func (c *Cluster) GetNodeID() string {
	return c.nodeID
}

// GetRegistry returns the registry (as interface for RPC compatibility)
func (c *Cluster) GetRegistry() interface{} {
	return c.registry
}

// syncLoop runs periodic sync tasks
func (c *Cluster) syncLoop() {
	ticker := time.NewTicker(c.broadcastInterval)
	defer ticker.Stop()

	for {
		select {
		case <-ticker.C:
			// Check for timeouts
			expired := c.registry.CheckTimeouts(c.timeout)
			for _, nodeID := range expired {
				c.logger.DiscoveryInfo("Node expired: %s", nodeID)
			}

			// Sync to disk
			if err := c.SyncToDisk(); err != nil {
				c.logger.DiscoveryError("Failed to sync config: %v", err)
			}

		case <-c.stopCh:
			return
		}
	}
}

// SyncToDisk saves the current state to state.toml (dynamic state only)
func (c *Cluster) SyncToDisk() error {
	c.mu.RLock()
	defer c.mu.RUnlock()

	// Build dynamic state from registry state
	dynamicState := &DynamicState{
		Cluster: ClusterMeta{
			ID:            "auto-discovered",
			AutoDiscovery: true,
			LastUpdated:   time.Now(),
		},
		LocalNode: NodeInfo{
			ID:           c.nodeID,
			Name:         c.nodeName,
			Address:      c.address,
			Role:         "worker",
			Capabilities: []string{},
		},
		Discovered: []PeerConfig{},
		LastSync:   time.Now(),
	}

	// Convert registry nodes to peer configs (only discovered peers, not self)
	nodes := c.registry.GetAll()
	for _, node := range nodes {
		// Skip self
		if node.ID == c.nodeID {
			continue
		}
		dynamicState.Discovered = append(dynamicState.Discovered, node.ToConfig())
	}

	// Save to state.toml
	return SaveDynamicState(c.dynamicStatePath, dynamicState)
}

// loadStaticConfig loads the static configuration (peers.toml)
func (c *Cluster) loadStaticConfig() error {
	staticConfig, err := LoadStaticConfig(c.staticConfigPath)
	if err != nil {
		return err
	}

	// Restore manually configured peers from static config to registry
	for _, peerConfig := range staticConfig.Peers {
		// Skip self
		if peerConfig.ID == c.nodeID {
			continue
		}

		// Only add enabled peers
		if !peerConfig.Enabled {
			continue
		}

		node := &Node{
			ID:           peerConfig.ID,
			Name:         peerConfig.Name,
			Address:      peerConfig.Address,
			Role:         peerConfig.Role,
			Capabilities: peerConfig.Capabilities,
			Priority:     peerConfig.Priority,
			Status:       NodeStatus(peerConfig.Status.State),
			LastSeen:     peerConfig.Status.LastSeen,
			LastError:    peerConfig.Status.LastError,
		}
		c.registry.AddOrUpdate(node)
	}

	return nil
}

// loadDynamicState loads the dynamic state (state.toml)
func (c *Cluster) loadDynamicState() error {
	dynamicState, err := LoadDynamicState(c.dynamicStatePath)
	if err != nil {
		return err
	}

	// Restore discovered peers from dynamic state to registry
	for _, peerConfig := range dynamicState.Discovered {
		// Skip self
		if peerConfig.ID == c.nodeID {
			continue
		}

		node := &Node{
			ID:           peerConfig.ID,
			Name:         peerConfig.Name,
			Address:      peerConfig.Address,
			Role:         peerConfig.Role,
			Capabilities: peerConfig.Capabilities,
			Priority:     peerConfig.Priority,
			Status:       NodeStatus(peerConfig.Status.State),
			LastSeen:     peerConfig.Status.LastSeen,
			LastError:    peerConfig.Status.LastError,
		}
		c.registry.AddOrUpdate(node)
	}

	return nil
}

// GetCapabilities returns all capabilities from all nodes
func (c *Cluster) GetCapabilities() []string {
	return c.registry.GetCapabilities()
}

// FindPeersByCapability returns nodes with a specific capability
func (c *Cluster) FindPeersByCapability(capability string) []*Node {
	return c.registry.FindByCapability(capability)
}

// GetOnlinePeers returns all online nodes as interface slice (for RPC compatibility)
func (c *Cluster) GetOnlinePeers() []interface{} {
	nodes := c.registry.GetOnline()
	result := make([]interface{}, len(nodes))
	for i, n := range nodes {
		result[i] = n
	}
	return result
}

// IsRunning returns true if the cluster is running
func (c *Cluster) IsRunning() bool {
	c.mu.RLock()
	defer c.mu.RUnlock()
	return c.running
}

// Call makes an RPC call to a peer
func (c *Cluster) Call(peerID, action string, payload map[string]interface{}) ([]byte, error) {
	if c.rpcClient == nil {
		return nil, fmt.Errorf("RPC client not initialized")
	}

	c.logger.RPCInfo("Calling %s: action=%s", peerID, action)
	return c.rpcClient.Call(peerID, action, payload)
}

// GetLogger returns the cluster logger
func (c *Cluster) GetLogger() *ClusterLogger {
	return c.logger
}

// GetAddress returns the RPC address of this node
func (c *Cluster) GetAddress() string {
	return c.address
}

// GetPeer returns a peer node by ID (for RPC client)
func (c *Cluster) GetPeer(peerID string) (interface{}, error) {
	node := c.registry.Get(peerID)
	if node == nil {
		return nil, fmt.Errorf("peer not found: %s", peerID)
	}
	return node, nil
}

// SetPorts sets the UDP and RPC ports
func (c *Cluster) SetPorts(udpPort, rpcPort int) {
	c.mu.Lock()
	defer c.mu.Unlock()
	c.udpPort = udpPort
	c.rpcPort = rpcPort
}

// GetPorts returns the configured UDP and RPC ports
func (c *Cluster) GetPorts() (udpPort, rpcPort int) {
	c.mu.RLock()
	defer c.mu.RUnlock()
	return c.udpPort, c.rpcPort
}

// LogInfo logs an info message (for discovery callback)
func (c *Cluster) LogInfo(msg string, args ...interface{}) {
	c.logger.DiscoveryInfo(msg, args...)
}

// LogError logs an error message (for discovery callback)
func (c *Cluster) LogError(msg string, args ...interface{}) {
	c.logger.DiscoveryError(msg, args...)
}

// LogDebug logs a debug message (for discovery callback)
func (c *Cluster) LogDebug(msg string, args ...interface{}) {
	c.logger.DiscoveryDebug(msg, args...)
}

// HandleDiscoveredNode handles a node discovered via UDP broadcast
func (c *Cluster) HandleDiscoveredNode(nodeID, name, address string, capabilities []string) {
	node := &Node{
		ID:           nodeID,
		Name:         name,
		Address:      address,
		Role:         "worker",
		Capabilities: capabilities,
		Priority:     1,
	}
	c.registry.AddOrUpdate(node)
}

// HandleNodeOffline handles a node going offline
func (c *Cluster) HandleNodeOffline(nodeID, reason string) {
	c.registry.MarkOffline(nodeID, reason)
}

// LogRPCInfo logs an RPC info message (for RPC callback)
func (c *Cluster) LogRPCInfo(msg string, args ...interface{}) {
	c.logger.RPCInfo(msg, args...)
}

// LogRPCError logs an RPC error message (for RPC callback)
func (c *Cluster) LogRPCError(msg string, args ...interface{}) {
	c.logger.RPCError(msg, args...)
}

// LogRPCDebug logs an RPC debug message (for RPC callback)
func (c *Cluster) LogRPCDebug(msg string, args ...interface{}) {
	c.logger.RPCDebug(msg, args...)
}
