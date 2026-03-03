// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package rpc

import (
	"encoding/json"
	"fmt"
	"net"
	"strings"
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
	LogRPCInfo(msg string, args ...interface{})
	LogRPCError(msg string, args ...interface{})
	LogRPCDebug(msg string, args ...interface{})
	GetPeer(peerID string) (interface{}, error) // Get peer directly
	GetLocalNetworkInterfaces() ([]LocalNetworkInterface, error) // Get local network interfaces
}

// LocalNetworkInterface represents a local network interface (for RPC interface)
type LocalNetworkInterface struct {
	IP   string
	Mask string
}

// Client handles RPC calls to other bots
type Client struct {
	cluster Cluster
	pool    *transport.Pool
	timeout time.Duration
}

// NewClient creates a new RPC client
func NewClient(cluster Cluster) *Client {
	return &Client{
		cluster: cluster,
		pool:    transport.NewPool(),
		timeout: 30 * time.Second,
	}
}

// Call makes an RPC call to a peer
func (c *Client) Call(peerID, action string, payload map[string]interface{}) ([]byte, error) {
	// Get peer using the new GetPeer method
	peerIface, err := c.cluster.GetPeer(peerID)
	if err != nil {
		return nil, fmt.Errorf("peer not found: %w", err)
	}

	peer, ok := peerIface.(Node)
	if !ok {
		return nil, fmt.Errorf("peer does not implement Node interface")
	}

	if !peer.IsOnline() {
		return nil, fmt.Errorf("peer is offline: %s", peerID)
	}

	// Get all addresses from the peer
	addresses := peer.GetAddresses()
	if len(addresses) == 0 {
		// Fallback to old Address field
		addresses = []string{peer.GetAddress()}
	}

	// Select the best address to connect to
	rpcPort := peer.GetRPCPort()
	if rpcPort == 0 {
		// Extract port from old Address field
		parts := strings.Split(peer.GetAddress(), ":")
		if len(parts) == 2 {
			rpcPort = 55555 // Default, won't be used if we extract correctly
			fmt.Sscanf(parts[1], "%d", &rpcPort)
		} else {
			rpcPort = 49200 // Default RPC port
		}
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

	// Select best address and try to connect
	selectedAddress, conn, err := c.connectToPeer(peerID, fullAddresses)
	if err != nil {
		return nil, fmt.Errorf("failed to connect to peer %s: %w", peerID, err)
	}

	c.cluster.LogRPCInfo("Connected to peer %s at %s (selected from %v)", peerID, selectedAddress, addresses)

	// Create request message
	req := transport.NewRequest(c.cluster.GetNodeID(), peerID, action, payload)
	req.Timestamp = time.Now().Unix()

	// Send request
	if err := conn.Send(req); err != nil {
		// Connection might be bad, remove it from pool
		c.pool.Remove(peerID, selectedAddress)
		return nil, fmt.Errorf("failed to send request: %w", err)
	}

	// Wait for response
	msg, err := c.receiveResponse(conn, req.ID)
	if err != nil {
		return nil, fmt.Errorf("failed to receive response: %w", err)
	}

	// Check for error
	if msg.Type == transport.RPCTypeError {
		return nil, fmt.Errorf("RPC error from peer: %s", msg.Error)
	}

	// Return payload
	if msg.Payload == nil {
		return []byte("{}"), nil
	}

	responseData, err := json.Marshal(msg.Payload)
	if err != nil {
		return nil, fmt.Errorf("failed to marshal response: %w", err)
	}

	return responseData, nil
}

// receiveResponse waits for a response message
func (c *Client) receiveResponse(conn *transport.Conn, messageID string) (*transport.RPCMessage, error) {
	timeoutCh := time.After(c.timeout)

	for {
		select {
		case <-time.After(100 * time.Millisecond):
			// Check if we got a message
			msg, err := conn.Receive()
			if err != nil {
				return nil, err
			}

			// Check if this is the response we're waiting for
			if msg.ID == messageID {
				return msg, nil
			}
			// Not our message, continue waiting

		case <-timeoutCh:
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
func (c *Client) connectToPeer(peerID string, addresses []string) (string, *transport.Conn, error) {
	if len(addresses) == 0 {
		return "", nil, fmt.Errorf("no addresses available")
	}

	// If only one address, try it directly
	if len(addresses) == 1 {
		return c.tryConnect(peerID, addresses[0])
	}

	// Multiple addresses: select best based on subnet matching
	bestAddress := c.selectBestAddress(addresses)
	c.cluster.LogRPCDebug("Selected address %s from %v for peer %s", bestAddress, addresses, peerID)

	// Try the best address first
	selectedAddr, conn, err := c.tryConnect(peerID, bestAddress)
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
		if addresses[i] == bestAddress {
			continue // Already tried
		}

		c.cluster.LogRPCDebug("Trying fallback address %s...", addresses[i])
		selectedAddr, conn, err := c.tryConnect(peerID, addresses[i])
		if err == nil {
			c.cluster.LogRPCInfo("Successfully connected to fallback address %s after primary failed", selectedAddr)
			return selectedAddr, conn, nil
		}
	}

	return "", nil, fmt.Errorf("all connection attempts failed for peer %s", peerID)
}

// tryConnect attempts to establish a connection to a specific address
func (c *Client) tryConnect(peerID, address string) (string, *transport.Conn, error) {
	conn, err := c.pool.Get(peerID, address)
	if err != nil {
		return address, nil, err
	}
	return address, conn, nil
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

// extractIP extracts IP address from "IP:Port" format
func extractIP(addr string) string {
	parts := strings.Split(addr, ":")
	if len(parts) >= 1 {
		return parts[0]
	}
	return ""
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
