// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package cluster

import (
	"context"
	"fmt"
	"net"
	"os"
	"path/filepath"
	"sync"
	"time"

	"github.com/276793422/NemesisBot/module/channels"
	"github.com/276793422/NemesisBot/module/cluster/discovery"
	"github.com/276793422/NemesisBot/module/cluster/handlers"
	"github.com/276793422/NemesisBot/module/cluster/rpc"
)

const (
	// DefaultUDPPort is the default UDP broadcast port
	DefaultUDPPort = 11949
	// DefaultRPCPort is the default WebSocket RPC port
	DefaultRPCPort = 21949
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
	role      string
	category  string
	tags      []string

	// Paths
	workspace       string
	staticConfigPath string  // peers.toml (static configuration)
	dynamicStatePath string  // state.toml (dynamic state)
	logDir          string

	// Components
	registry   *Registry
	logger     *ClusterLogger
	discovery  *discovery.Discovery
	rpcClient  *rpc.Client
	rpcServer  *rpc.Server      // RPC server instance
	rpcChannel *channels.RPCChannel // RPC channel for LLM communication

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
		udpPort:          DefaultUDPPort,  // Default UDP port
		rpcPort:          DefaultRPCPort,  // Default RPC port
		broadcastInterval: DefaultBroadcastInterval,
		timeout:          DefaultTimeout,
		stopCh:           make(chan struct{}),
		role:             "worker",  // Default role
		category:         "general", // Default category
		tags:             []string{},// Default tags
	}

	// Load static config to get local node info (role, category, tags)
	if err := cluster.loadStaticConfig(); err != nil {
		logger.DiscoveryError("Failed to load static config: %v", err)
		// Continue anyway, will use defaults
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

	// Find available UDP port
	actualUDPPort, err := findAvailablePort(c.udpPort, "udp")
	if err != nil {
		return fmt.Errorf("failed to find available UDP port: %w", err)
	}
	if actualUDPPort != c.udpPort {
		c.logger.DiscoveryInfo("UDP port %d unavailable, using %d", c.udpPort, actualUDPPort)
		c.udpPort = actualUDPPort
	}

	// Find available RPC port
	actualRPCPort, err := findAvailablePort(c.rpcPort, "tcp")
	if err != nil {
		return fmt.Errorf("failed to find available RPC port: %w", err)
	}
	if actualRPCPort != c.rpcPort {
		c.logger.DiscoveryInfo("RPC port %d unavailable, using %d", c.rpcPort, actualRPCPort)
		c.rpcPort = actualRPCPort
	}

	// Set RPC address to bind all interfaces
	// Other nodes can connect using any IP that reaches this machine
	c.address = fmt.Sprintf("0.0.0.0:%d", c.rpcPort)

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

	// Create and start RPC server
	c.rpcServer = rpc.NewServer(c)
	if err := c.rpcServer.Start(c.rpcPort); err != nil {
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

	// Stop RPC server
	if c.rpcServer != nil {
		if err := c.rpcServer.Stop(); err != nil {
			c.logger.RPCError("Failed to stop RPC server: %v", err)
		}
	}

	// Stop RPC channel
	if c.rpcChannel != nil {
		ctx := context.Background()
		if err := c.rpcChannel.Stop(ctx); err != nil {
			c.logger.RPCError("Failed to stop RPC channel: %v", err)
		}
		c.rpcChannel = nil
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

// GetRPCChannel returns the RPC channel (may be nil if not configured)
func (c *Cluster) GetRPCChannel() *channels.RPCChannel {
	c.mu.RLock()
	defer c.mu.RUnlock()
	return c.rpcChannel
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

	// Load local node information from static config
	// Check if the node in static config matches our generated nodeID
	if staticConfig.Node.ID == c.nodeID || staticConfig.Node.ID == "" {
		// Use our generated nodeID, but load other info from config
		if staticConfig.Node.Name != "" {
			c.nodeName = staticConfig.Node.Name
		}
		if staticConfig.Node.Role != "" {
			c.role = staticConfig.Node.Role
		}
		if staticConfig.Node.Category != "" {
			c.category = staticConfig.Node.Category
		}
		if len(staticConfig.Node.Tags) > 0 {
			c.tags = staticConfig.Node.Tags
		}
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
			Addresses:    peerConfig.Addresses,
			RPCPort:      peerConfig.RPCPort,
			Role:         peerConfig.Role,
			Category:     peerConfig.Category,
			Tags:         peerConfig.Tags,
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
			Addresses:    peerConfig.Addresses,
			RPCPort:      peerConfig.RPCPort,
			Role:         peerConfig.Role,
			Category:     peerConfig.Category,
			Tags:         peerConfig.Tags,
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
	return c.CallWithContext(context.Background(), peerID, action, payload)
}

// CallWithContext makes an RPC call to a peer with context support for cancellation and timeout
func (c *Cluster) CallWithContext(ctx context.Context, peerID, action string, payload map[string]interface{}) ([]byte, error) {
	if c.rpcClient == nil {
		return nil, fmt.Errorf("RPC client not initialized")
	}

	// Check if context is already cancelled
	select {
	case <-ctx.Done():
		return nil, ctx.Err()
	default:
	}

	c.logger.RPCInfo("Calling %s: action=%s", peerID, action)
	return c.rpcClient.CallWithContext(ctx, peerID, action, payload)
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

// GetLocalNetworkInterfaces returns local network interfaces (for RPC client)
func (c *Cluster) GetLocalNetworkInterfaces() ([]rpc.LocalNetworkInterface, error) {
	interfaces, err := GetLocalNetworkInterfaces()
	if err != nil {
		return nil, err
	}

	result := make([]rpc.LocalNetworkInterface, len(interfaces))
	for i, iface := range interfaces {
		result[i] = rpc.LocalNetworkInterface{
			IP:   iface.IP,
			Mask: iface.Mask,
		}
	}
	return result, nil
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

// GetRPCPort returns the RPC port (for discovery callback)
func (c *Cluster) GetRPCPort() int {
	c.mu.RLock()
	defer c.mu.RUnlock()
	return c.rpcPort
}

// GetAllLocalIPs returns all local IP addresses (for discovery callback)
func (c *Cluster) GetAllLocalIPs() []string {
	ips, err := GetAllLocalIPs()
	if err != nil {
		c.logger.DiscoveryError("Failed to get local IPs: %v", err)
		// Fallback to single IP from address
		return []string{c.address}
	}
	return ips
}

// GetRole returns the node role (for discovery callback)
func (c *Cluster) GetRole() string {
	return c.role
}

// GetCategory returns the node category (for discovery callback)
func (c *Cluster) GetCategory() string {
	return c.category
}

// GetTags returns the node tags (for discovery callback)
func (c *Cluster) GetTags() []string {
	return c.tags
}

// HandleDiscoveredNode handles a node discovered via UDP broadcast
func (c *Cluster) HandleDiscoveredNode(nodeID, name string, addresses []string, rpcPort int, role, category string, tags []string, capabilities []string) {
	// For backward compatibility, use the first address as primary Address
	// Note: This might not be the best choice if the first address is unreachable
	// The RPC client will try all addresses in the addresses array anyway
	primaryAddress := ""
	if len(addresses) > 0 {
		primaryAddress = fmt.Sprintf("%s:%d", addresses[0], rpcPort)
	}

	// TODO: Could implement smart address selection here:
	// - Try to connect to each address
	// - Use the first reachable one as primary
	// - This requires async handling which might complicate discovery

	node := &Node{
		ID:           nodeID,
		Name:         name,
		Address:      primaryAddress,  // Primary address for backward compatibility
		Addresses:    addresses,       // All addresses - RPC client will try all of them
		RPCPort:      rpcPort,
		Role:         role,
		Category:     category,
		Tags:         tags,
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

// RegisterRPCHandler registers an RPC handler for an action
// This allows external code to register custom RPC handlers
func (c *Cluster) RegisterRPCHandler(action string, handler func(payload map[string]interface{}) (map[string]interface{}, error)) error {
	c.mu.RLock()
	if !c.running {
		c.mu.RUnlock()
		return fmt.Errorf("cluster is not running")
	}
	if c.rpcServer == nil {
		c.mu.RUnlock()
		return fmt.Errorf("RPC server is not initialized")
	}
	c.mu.RUnlock()

	c.rpcServer.RegisterHandler(action, handler)
	c.logger.RPCInfo("Registered RPC handler for action: %s", action)
	return nil
}

// SetRPCChannel sets the RPC channel and triggers LLM handler registration
// This is called by loop.go after creating the RPCChannel
//
// Thread safety: This method uses lock-free pattern to avoid deadlock:
// - Acquires lock only to set c.rpcChannel and read state
// - Releases lock before calling registerPeerChatHandlers()
// - This avoids deadlock: registerPeerChatHandlers() internally calls RegisterRPCHandler()
//   which tries to acquire a read lock while we might be holding a write lock
//
// There's a tiny race window between Unlock() and registerPeerChatHandlers() where
// Stop() or server shutdown could occur. This is acceptable as:
// - It's extremely short (microseconds)
// - Worst case: LLM handlers don't get registered, but no deadlock occurs
// - RegisterRPCHandler() has its own state checks and will return error if not running
func (c *Cluster) SetRPCChannel(rpcCh *channels.RPCChannel) {
	// Step 1: Acquire lock to set rpcChannel
	c.mu.Lock()
	c.rpcChannel = rpcCh

	// Step 2: Save state (avoid reading outside of lock)
	wasRunning := c.running
	hasServer := c.rpcServer != nil

	// Step 3: Release lock BEFORE calling registerPeerChatHandlers
	// This prevents deadlock: registerPeerChatHandlers -> RegisterRPCHandler -> c.mu.RLock()
	c.mu.Unlock()

	// Step 4: Call registerPeerChatHandlers outside of lock
	// RegisterRPCHandler will acquire its own read lock for safety checks
	if wasRunning && hasServer {
		c.registerPeerChatHandlers()
	}
}

// registerPeerChatHandlers registers LLM-related handlers when RPCChannel is ready
// This must be called after both RPC Server and RPC Channel are initialized
func (c *Cluster) registerPeerChatHandlers() {
	if c.rpcChannel == nil {
		c.logger.RPCInfo("RPCChannel not ready, skipping LLM handler registration")
		return
	}

	// Create a registrar function that forwards to RegisterRPCHandler
	registrar := func(action string, handler func(map[string]interface{}) (map[string]interface{}, error)) {
		if err := c.RegisterRPCHandler(action, handler); err != nil {
			c.logger.RPCError("Failed to register handler '%s': %v", action, err)
		}
	}

	// Create a handler factory that creates peer chat handler
	handlerFactory := func(rpcChannel *channels.RPCChannel) func(map[string]interface{}) (map[string]interface{}, error) {
		handler := rpc.NewPeerChatHandler(c, rpcChannel)
		return handler.Handle
	}

	// Register peer chat handlers using the handlers package
	handlers.RegisterPeerChatHandlers(c.logger, c.rpcChannel, handlerFactory, registrar)

	// Register custom handlers (hello, etc.)
	handlers.RegisterCustomHandlers(c.logger, c.GetNodeID, registrar)
}

// RegisterBasicHandlers registers basic RPC handlers (default and custom)
// This can be called directly in daemon mode where RPCChannel is not available
func (c *Cluster) RegisterBasicHandlers() error {
	c.mu.RLock()
	serverRunning := c.running
	c.mu.RUnlock()

	if !serverRunning {
		return fmt.Errorf("cluster not running")
	}

	// Create a registrar function that forwards to RegisterRPCHandler
	registrar := func(action string, handler func(map[string]interface{}) (map[string]interface{}, error)) {
		if err := c.RegisterRPCHandler(action, handler); err != nil {
			c.logger.RPCError("Failed to register handler '%s': %v", action, err)
		}
	}

	// Register default handlers (ping, get_capabilities, get_info, list_actions)
	handlers.RegisterDefaultHandlers(
		c.logger,
		c.GetNodeID,
		c.GetCapabilities,
		c.GetOnlinePeers,
		c.GetActionsSchema,
		registrar,
	)

	// Register custom handlers (hello, etc.)
	handlers.RegisterCustomHandlers(c.logger, c.GetNodeID, registrar)

	return nil
}

// findAvailablePort finds an available port starting from the given port
// It tries port, port+1, port+2, ... until it finds an available one
// Returns the available port and nil error, or 0 and error if no port available
func findAvailablePort(startPort int, protocol string) (int, error) {
	maxAttempts := 100 // Try at most 100 ports

	for i := 0; i < maxAttempts; i++ {
		port := startPort + i
		addr := fmt.Sprintf(":%d", port)

		var err error
		if protocol == "udp" {
			// For UDP, use ListenPacket
			conn, err := net.ListenPacket("udp", addr)
			if err == nil {
				conn.Close()
				return port, nil
			}
		} else {
			// For TCP, use Listen
			listener, err := net.Listen("tcp", addr)
			if err == nil {
				listener.Close()
				return port, nil
			}
		}

		// If error, try next port
		_ = err
	}

	return 0, fmt.Errorf("no available port found starting from %d", startPort)
}
