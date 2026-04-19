// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package rpc

import (
	"context"
	"encoding/json"
	"fmt"
	"net"
	"strings"
	"sync"
	"time"

	"github.com/276793422/NemesisBot/module/cluster/transport"
)

// Node represents a node in the cluster (minimal interface)
type Node interface {
	GetID() string
	GetName() string
	GetAddress() string
	GetAddresses() []string
	GetRPCPort() int
	GetCapabilities() []string
	GetStatus() string
	IsOnline() bool
}

// Logger represents logging functions
type Logger interface {
	RPCInfo(format string, args ...interface{})
	RPCError(format string, args ...interface{})
	RPCDebug(format string, args ...interface{})
}

// Cluster represents the cluster interface needed by RPC
type Cluster interface {
	GetRegistry() interface{}
	GetNodeID() string
	GetAddress() string
	GetCapabilities() []string
	GetOnlinePeers() []interface{}
	GetActionsSchema() []interface{} // Get all available actions with schema
	LogRPCInfo(msg string, args ...interface{})
	LogRPCError(msg string, args ...interface{})
	LogRPCDebug(msg string, args ...interface{})
	GetPeer(peerID string) (interface{}, error)                  // Get peer directly
	GetLocalNetworkInterfaces() ([]LocalNetworkInterface, error) // Get local network interfaces
	CallWithContext(ctx context.Context, peerID, action string, payload map[string]interface{}) ([]byte, error) // RPC call
	GetTaskResultStorer() TaskResultStorer                       // H4: B-side task result persistence
}

// TaskResultStorer B 端任务结果持久化接口（避免循环依赖，在 rpc 包中定义）
type TaskResultStorer interface {
	SetRunning(taskID, sourceNode string)
	SetResult(taskID, resultStatus, response, errMsg, sourceNode string) error
	Delete(taskID string) error
}

// LocalNetworkInterface represents a local network interface (for RPC interface)
type LocalNetworkInterface struct {
	IP   string
	Mask string
}

// RateLimiter limits the rate of RPC calls
type RateLimiter struct {
	mu          sync.Mutex
	tokens      map[string]int         // peer_id -> token count
	lastRefill  time.Time              // last refill time
	maxTokens   int                    // tokens per refill
	refillRate  time.Duration          // refill interval
	requests    map[string][]time.Time // peer_id -> request timestamps (for burst detection)
	maxRequests int                    // max requests per peer per window
	window      time.Duration          // sliding window duration
}

// NewRateLimiter creates a new rate limiter
func NewRateLimiter(maxTokens int, refillRate time.Duration, maxRequests int, window time.Duration) *RateLimiter {
	return &RateLimiter{
		tokens:      make(map[string]int),
		lastRefill:  time.Now(),
		maxTokens:   maxTokens,
		refillRate:  refillRate,
		requests:    make(map[string][]time.Time),
		maxRequests: maxRequests,
		window:      window,
	}
}

// Acquire acquires a token for RPC call to peerID
func (rl *RateLimiter) Acquire(ctx context.Context, peerID string) error {
	// Initialize peer tokens if not exists
	rl.mu.Lock()
	if _, exists := rl.tokens[peerID]; !exists {
		rl.tokens[peerID] = rl.maxTokens
		rl.requests[peerID] = []time.Time{}
	}
	rl.mu.Unlock()

	// Refill tokens periodically
	rl.mu.Lock()
	if time.Since(rl.lastRefill) > rl.refillRate {
		rl.lastRefill = time.Now()
		// Refill tokens for all peers
		for peer := range rl.tokens {
			rl.tokens[peer] = rl.maxTokens
		}
	}
	rl.mu.Unlock()

	// Check sliding window rate limit
	rl.mu.Lock()
	now := time.Now()
	windowStart := now.Add(-rl.window)

	// Clean old timestamps and count recent requests
	if rl.requests[peerID] != nil {
		validRequests := rl.requests[peerID][:0]
		for _, ts := range rl.requests[peerID] {
			if ts.After(windowStart) {
				validRequests = append(validRequests, ts)
			}
		}
		rl.requests[peerID] = validRequests
	}

	// Check if peer has exceeded burst rate
	if len(rl.requests[peerID]) >= rl.maxRequests {
		oldestAllowed := now.Add(-rl.window)
		if rl.requests[peerID][0].After(oldestAllowed) {
			rl.mu.Unlock()
			return fmt.Errorf("peer %s rate limited: too many requests (max=%d per %v)", peerID, rl.maxRequests, rl.window)
		}
	}
	rl.mu.Unlock()

	// Acquire token with retry logic to avoid holding lock during wait
	for {
		rl.mu.Lock()

		// Refill tokens again in case they were refilled while waiting
		if time.Since(rl.lastRefill) > rl.refillRate {
			rl.lastRefill = time.Now()
			for peer := range rl.tokens {
				rl.tokens[peer] = rl.maxTokens
			}
		}

		if rl.tokens[peerID] > 0 {
			// Token available, acquire it
			rl.tokens[peerID]--

			// Record request timestamp for sliding window
			rl.requests[peerID] = append(rl.requests[peerID], time.Now())

			rl.mu.Unlock()
			return nil
		}

		// No token available, release lock and wait
		rl.mu.Unlock()

		// Wait outside of lock
		select {
		case <-ctx.Done():
			return ctx.Err()
		case <-time.After(100 * time.Millisecond):
			// Retry after refill interval
			continue
		}
	}
}

// Release releases a token after RPC call completes
func (rl *RateLimiter) Release(peerID string) {
	rl.mu.Lock()
	defer rl.mu.Unlock()
	rl.tokens[peerID]++
}

// Client handles RPC calls to other bots
type Client struct {
	cluster     Cluster
	pool        *transport.Pool
	rateLimiter *RateLimiter
	timeout     time.Duration
	authToken   string // RPC authentication token
}

// NewClient creates a new RPC client
func NewClient(cluster Cluster) *Client {
	return &Client{
		cluster: cluster,
		pool:    transport.NewPool(),
		rateLimiter: NewRateLimiter(
			10,             // maxTokens: 10 calls per second per peer
			1*time.Second,  // refillRate: refill every second
			30,             // maxRequests: 30 requests per peer per sliding window
			10*time.Second, // window: sliding window of 10 seconds
		),
		timeout:   60 * time.Minute, // RPC Client timeout: 60 minutes (outermost timeout)
	}
}

// SetAuthToken sets the authentication token for RPC connections
func (c *Client) SetAuthToken(token string) {
	c.authToken = token
	// Also set token on the pool
	if c.pool != nil {
		c.pool.SetAuthToken(token)
	}
}

// Call makes an RPC call to a peer (deprecated - use CallWithContext for better timeout control)
func (c *Client) Call(peerID, action string, payload map[string]interface{}) ([]byte, error) {
	return c.CallWithContext(context.Background(), peerID, action, payload)
}

// CallWithContext makes an RPC call to a peer with context support
func (c *Client) CallWithContext(ctx context.Context, peerID, action string, payload map[string]interface{}) ([]byte, error) {
	// Check if context is already cancelled
	select {
	case <-ctx.Done():
		return nil, ctx.Err()
	default:
	}

	// Apply rate limiting with context
	if err := c.rateLimiter.Acquire(ctx, peerID); err != nil {
		return nil, fmt.Errorf("rate limit error: %w", err)
	}
	defer c.rateLimiter.Release(peerID)

	// Get peer using the new GetPeer method
	peerIface, err := c.cluster.GetPeer(peerID)
	if err != nil {
		c.cluster.LogRPCError("Peer not found: %s", peerID)
		return nil, fmt.Errorf("peer not found: %w", err)
	}
	c.cluster.LogRPCInfo("Found peer %s", peerID)

	peer, ok := peerIface.(Node)
	if !ok {
		c.cluster.LogRPCError("Peer does not implement Node interface: %s", peerID)
		return nil, fmt.Errorf("peer does not implement Node interface")
	}

	if !peer.IsOnline() {
		c.cluster.LogRPCError("Peer is offline: %s", peerID)
		return nil, fmt.Errorf("peer is offline: %s", peerID)
	}
	c.cluster.LogRPCInfo("Peer %s is online", peerID)

	// Get all addresses from the peer
	addresses := peer.GetAddresses()
	c.cluster.LogRPCInfo("Peer %s addresses: %v (len=%d)", peerID, addresses, len(addresses))
	if len(addresses) == 0 {
		// Fallback to old Address field
		addresses = []string{peer.GetAddress()}
		c.cluster.LogRPCInfo("Using fallback address: %v", addresses)
	}

	// Select the best address to connect to
	rpcPort := peer.GetRPCPort()
	c.cluster.LogRPCInfo("Peer %s RPCPort: %d", peerID, rpcPort)
	if rpcPort == 0 {
		// Extract port from old Address field
		parts := strings.Split(peer.GetAddress(), ":")
		if len(parts) == 2 {
			rpcPort = 55555 // Default, won't be used if we extract correctly
			fmt.Sscanf(parts[1], "%d", &rpcPort)
		} else {
			rpcPort = 21949 // Default RPC port
		}
		c.cluster.LogRPCInfo("Extracted RPC port: %d for peer %s", rpcPort, peerID)
	}

	// Build full addresses (IP:Port)
	fullAddresses := make([]string, 0, len(addresses))
	for _, addr := range addresses {
		if !strings.Contains(addr, ":") {
			fullAddresses = append(fullAddresses, fmt.Sprintf("%s:%d", addr, rpcPort))
		} else {
			fullAddresses = append(fullAddresses, addr)
		}
	}
	c.cluster.LogRPCInfo("Attempting to connect to peer %s at %v", peerID, fullAddresses)

	// Select best address and try to connect
	//
	// Connection strategy: each CallWithContext dials a new, exclusive TCP connection.
	// This avoids the concurrent response mismatch bug where multiple callers sharing
	// the same pooled connection could consume each other's messages from recvChan.
	//
	// Trade-off: adds ~1 TCP handshake per call (negligible vs LLM processing time).
	// This is safe for the current scale (a few nodes, low-frequency RPC).
	//
	// Future upgrade path (if high-frequency cross-subnet RPC becomes a bottleneck):
	//   Implement a "pending calls map" dispatcher — one goroutine per connection reads
	//   recvChan and routes each message to the correct waiter by messageID. This enables
	//   true connection multiplexing without message stealing. See: docs/PLAN/2026-04-19_CLUSTER_HIGH_SEVERITY_FIXES.md
	selectedAddress, conn, err := c.connectToPeer(ctx, peerID, fullAddresses)
	if err != nil {
		c.cluster.LogRPCError("Failed to connect to peer %s: %v", peerID, err)
		return nil, fmt.Errorf("failed to connect to peer %s: %w", peerID, err)
	}
	// Connection is exclusive to this call — close when done
	defer conn.Close()

	c.cluster.LogRPCInfo("Connected to peer %s at %s", peerID, selectedAddress)

	// Create request message
	req := transport.NewRequest(c.cluster.GetNodeID(), peerID, action, payload)
	req.Timestamp = time.Now().Unix()

	// Send request
	c.cluster.LogRPCInfo("Sending request action=%s to peer %s (id=%s)", req.Action, peerID, req.ID)
	if err := conn.Send(req); err != nil {
		c.cluster.LogRPCError("Failed to send request to peer %s: %v", peerID, err)
		return nil, fmt.Errorf("failed to send request: %w", err)
	}
	c.cluster.LogRPCInfo("Request sent successfully to peer %s, waiting for response (id=%s)", peerID, req.ID)

	// Wait for response with context timeout
	response, err := c.receiveResponseWithContext(ctx, conn, req.ID)
	if err != nil {
		c.cluster.LogRPCError("Failed to receive response from %s for %s: %v", peerID, req.ID, err)
		return nil, fmt.Errorf("failed to receive response: %w", err)
	}

	c.cluster.LogRPCInfo("Received response from %s: type=%s, id=%s", peerID, response.Type, response.ID)

	// Check for error
	if response.Type == transport.RPCTypeError {
		return nil, fmt.Errorf("RPC error from peer: %s", response.Error)
	}

	// Return payload
	if response.Payload == nil {
		return []byte("{}"), nil
	}

	responseData, err := json.Marshal(response.Payload)
	if err != nil {
		return nil, fmt.Errorf("failed to marshal response: %w", err)
	}

	return responseData, nil
}

// receiveResponse waits for a response message
func (c *Client) receiveResponse(conn *transport.TCPConn, messageID string) (*transport.RPCMessage, error) {
	c.cluster.LogRPCDebug("Waiting for response message ID: %s", messageID)

	// Use background context for timeout
	ctx, cancel := context.WithTimeout(context.Background(), c.timeout)
	defer cancel()

	return c.receiveResponseWithContext(ctx, conn, messageID)
}

// receiveResponseWithContext waits for a response message with context support
func (c *Client) receiveResponseWithContext(ctx context.Context, conn *transport.TCPConn, messageID string) (*transport.RPCMessage, error) {
	c.cluster.LogRPCDebug("Waiting for response message ID: %s (with context)", messageID)

	// Calculate deadline based on client timeout
	deadline := time.Now().Add(c.timeout)

	// Check if context has a deadline, use the earlier one
	if ctxDeadline, ok := ctx.Deadline(); ok {
		if ctxDeadline.Before(deadline) {
			deadline = ctxDeadline
		}
	}

	// Create timeout channel
	timeout := time.After(time.Until(deadline))

	for {
		// Check context cancellation
		select {
		case <-ctx.Done():
			c.cluster.LogRPCDebug("Context cancelled while waiting for response: %v", ctx.Err())
			return nil, fmt.Errorf("context cancelled: %w", ctx.Err())
		default:
		}

		// Wait for message or timeout
		select {
		case msg, ok := <-conn.Receive():
			if !ok {
				c.cluster.LogRPCError("Connection closed while waiting for response")
				return nil, fmt.Errorf("connection closed")
			}

			if msg == nil {
				continue // Skip nil messages
			}

			// Log every message received
			c.cluster.LogRPCDebug("Received message: id=%s, type=%s, waiting_for=%s", msg.ID, msg.Type, messageID)

			// Check if this is the response we're waiting for
			if msg.ID == messageID {
				c.cluster.LogRPCDebug("Message ID matched! Returning response")
				return msg, nil
			}
			// Not our message, continue waiting
			c.cluster.LogRPCDebug("Message ID mismatch, continuing to wait...")

		case <-timeout:
			c.cluster.LogRPCError("Timeout waiting for response (ID: %s)", messageID)
			return nil, fmt.Errorf("timeout waiting for response")
		}
	}
}

// Close closes the client and all connections
func (c *Client) Close() error {
	return c.pool.Close()
}

// connectToPeer selects the best address and attempts to connect
// Returns the selected address and connection, or error if all attempts fail
func (c *Client) connectToPeer(ctx context.Context, peerID string, addresses []string) (string, *transport.TCPConn, error) {
	if len(addresses) == 0 {
		return "", nil, fmt.Errorf("no addresses available")
	}

	// If only one address, try it directly
	if len(addresses) == 1 {
		return c.tryConnect(ctx, peerID, addresses[0])
	}

	// Multiple addresses: select best based on subnet matching
	bestAddress := c.selectBestAddress(addresses)
	c.cluster.LogRPCDebug("Selected address %s from %v for peer %s", bestAddress, addresses, peerID)

	// Try the best address first
	selectedAddr, conn, err := c.tryConnect(ctx, peerID, bestAddress)
	if err == nil {
		return selectedAddr, conn, nil
	}

	c.cluster.LogRPCDebug("Failed to connect to %s, trying other addresses...", bestAddress)

	// Fallback: try other addresses (limit to first 3 to avoid long delays)
	maxTries := len(addresses)
	if maxTries > 4 {
		maxTries = 4 // Best address + 3 fallback attempts
	}

	for i := 0; i < maxTries && i < len(addresses); i++ {
		// Check context before each attempt
		select {
		case <-ctx.Done():
			return "", nil, ctx.Err()
		default:
		}

		if addresses[i] == bestAddress {
			continue // Already tried
		}

		c.cluster.LogRPCDebug("Trying fallback address %s...", addresses[i])
		selectedAddr, conn, err := c.tryConnect(ctx, peerID, addresses[i])
		if err == nil {
			c.cluster.LogRPCInfo("Successfully connected to fallback address %s after primary failed", selectedAddr)
			return selectedAddr, conn, nil
		}
	}

	return "", nil, fmt.Errorf("all connection attempts failed for peer %s", peerID)
}

// tryConnect dials a new, exclusive TCP connection to the given address.
// The connection is NOT added to the pool — the caller owns it and must close it.
func (c *Client) tryConnect(ctx context.Context, peerID, address string) (string, *transport.TCPConn, error) {
	select {
	case <-ctx.Done():
		return address, nil, ctx.Err()
	default:
	}

	c.cluster.LogRPCDebug("Dialing exclusive connection to %s (peer=%s)", address, peerID)

	dialer := net.Dialer{Timeout: 10 * time.Second}
	netConn, err := dialer.DialContext(ctx, "tcp", address)
	if err != nil {
		return address, nil, err
	}

	config := &transport.TCPConnConfig{
		NodeID:         peerID,
		Address:        address,
		ReadBufferSize: 100,
		SendBufferSize: 100,
		SendTimeout:    10 * time.Second,
		IdleTimeout:    0, // No idle timeout — connection lifetime controlled by caller
		AuthToken:      c.authToken,
	}
	tcpConn := transport.NewTCPConn(netConn, config)
	tcpConn.Start()

	c.cluster.LogRPCDebug("Exclusive connection established to %s", address)
	return address, tcpConn, nil
}

// selectBestAddress selects the best address from a list
// Uses subnet matching with local network interfaces
func (c *Client) selectBestAddress(addresses []string) string {
	if len(addresses) == 0 {
		return ""
	}
	if len(addresses) == 1 {
		return addresses[0]
	}

	// Get local network interfaces for subnet matching
	localInterfaces, err := c.cluster.GetLocalNetworkInterfaces()
	if err != nil || len(localInterfaces) == 0 {
		// Fallback: return first address
		return addresses[0]
	}

	// Try to find an address in the same subnet as any local interface
	for _, remoteAddr := range addresses {
		// Extract IP from "IP:Port" format
		remoteIP := extractIP(remoteAddr)
		if remoteIP == "" {
			continue
		}

		// Check if this remote IP is in the same subnet as any local interface
		for _, localIface := range localInterfaces {
			if isSameSubnet(remoteIP, localIface.IP, localIface.Mask) {
				c.cluster.LogRPCDebug("Address %s is in same subnet as local %s", remoteAddr, localIface.IP)
				return remoteAddr
			}
		}
	}

	// No subnet match found, return first address
	c.cluster.LogRPCDebug("No subnet match found, using first address: %s", addresses[0])
	return addresses[0]
}

// extractIP extracts IP address from "IP:Port" or "[IPv6]:Port" format
func extractIP(addr string) string {
	host, _, err := net.SplitHostPort(addr)
	if err != nil {
		return addr
	}
	return host
}

// isSameSubnet checks if two IPs are in the same subnet given a mask
func isSameSubnet(ip1, ip2, mask string) bool {
	parsedIP1 := net.ParseIP(ip1)
	parsedIP2 := net.ParseIP(ip2)
	if parsedIP1 == nil || parsedIP2 == nil {
		return false
	}

	parsedMask := net.ParseIP(mask)
	if parsedMask == nil {
		return false
	}

	// Convert to IPMask
	ipMask := net.IPMask(parsedMask)

	// Apply mask to both IPs
	network1 := parsedIP1.Mask(ipMask)
	network2 := parsedIP2.Mask(ipMask)

	// Check if they're in the same network
	return network1.String() == network2.String()
}
