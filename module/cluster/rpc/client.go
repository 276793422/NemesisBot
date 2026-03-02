// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package rpc

import (
	"encoding/json"
	"fmt"
	"time"

	"github.com/276793422/NemesisBot/module/cluster/transport"
)

// Node represents a node in the cluster (minimal interface)
type Node interface {
	GetID() string
	GetName() string
	GetAddress() string
	GetCapabilities() []string
	GetStatus() string
	IsOnline() bool
}

// Registry represents the node registry (minimal interface)
type Registry interface {
	Get(nodeID string) interface{}
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
	// Get peer from registry
	registryIface := c.cluster.GetRegistry()
	registry, ok := registryIface.(Registry)
	if !ok {
		return nil, fmt.Errorf("invalid registry type")
	}

	peerIface := registry.Get(peerID)
	if peerIface == nil {
		return nil, fmt.Errorf("peer not found: %s", peerID)
	}

	peer, ok := peerIface.(Node)
	if !ok {
		return nil, fmt.Errorf("peer is not a valid Node: %s", peerID)
	}

	if !peer.IsOnline() {
		return nil, fmt.Errorf("peer is offline: %s", peerID)
	}

	// Get or create connection
	conn, err := c.pool.Get(peerID, peer.GetAddress())
	if err != nil {
		return nil, fmt.Errorf("failed to get connection: %w", err)
	}

	// Create request message
	req := transport.NewRequest(c.cluster.GetNodeID(), peerID, action, payload)
	req.Timestamp = time.Now().Unix()

	// Send request
	if err := conn.Send(req); err != nil {
		// Connection might be bad, remove from pool
		c.pool.Remove(peerID)
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
