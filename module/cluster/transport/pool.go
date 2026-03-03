// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package transport

import (
	"encoding/json"
	"fmt"
	"net"
	"net/url"
	"sync"
	"time"

	"github.com/gorilla/websocket"
)

// Pool manages a pool of WebSocket connections
type Pool struct {
	mu        sync.RWMutex
	conns     map[string]*Conn // node_id -> Conn
	dialer    *websocket.Dialer
	timeout   time.Duration
}

// Conn represents a WebSocket connection
type Conn struct {
	mu        sync.RWMutex
	ws        *websocket.Conn
	nodeID    string
	address   string
	createdAt time.Time
	lastUsed  time.Time
}

// NewPool creates a new connection pool
func NewPool() *Pool {
	return &Pool{
		conns: make(map[string]*Conn),
		dialer: &websocket.Dialer{
			HandshakeTimeout: 10 * time.Second,
		},
		timeout: 10 * time.Second,
	}
}

// Get gets or creates a connection to a node
func (p *Pool) Get(nodeID, address string) (*Conn, error) {
	// Use nodeID:address as the cache key to support multiple addresses per node
	key := nodeID + ":" + address

	p.mu.RLock()
	conn, exists := p.conns[key]
	p.mu.RUnlock()

	if exists && conn.IsActive() {
		conn.UpdateLastUsed()
		return conn, nil
	}

	// Create new connection
	p.mu.Lock()
	defer p.mu.Unlock()

	// Double-check after acquiring write lock
	if conn, exists := p.conns[key]; exists && conn.IsActive() {
		return conn, nil
	}

	// Dial WebSocket
	ws, err := p.dial(address)
	if err != nil {
		return nil, fmt.Errorf("failed to dial %s: %w", address, err)
	}

	newConn := &Conn{
		ws:        ws,
		nodeID:    nodeID,
		address:   address,
		createdAt: time.Now(),
		lastUsed:  time.Now(),
	}

	p.conns[key] = newConn

	return newConn, nil
}

// Remove removes a connection from the pool
// If address is empty, removes all connections for the node
func (p *Pool) Remove(nodeID, address string) {
	p.mu.Lock()
	defer p.mu.Unlock()

	if address == "" {
		// Remove all connections for this node
		for key, conn := range p.conns {
			if conn.GetNodeID() == nodeID {
				conn.Close()
				delete(p.conns, key)
			}
		}
	} else {
		// Remove specific connection
		key := nodeID + ":" + address
		if conn, ok := p.conns[key]; ok {
			conn.Close()
			delete(p.conns, key)
		}
	}
}

// dial creates a new WebSocket connection
func (p *Pool) dial(address string) (*websocket.Conn, error) {
	// Parse address
	u := url.URL{
		Scheme: "ws",
		Host:   address,
		Path:   "/rpc",
	}

	// Add timeout
	conn, _, err := p.dialer.Dial(u.String(), nil)
	if err != nil {
		return nil, err
	}

	return conn, nil
}

// Close closes all connections in the pool
func (p *Pool) Close() error {
	p.mu.Lock()
	defer p.mu.Unlock()

	var errs []error

	for _, conn := range p.conns {
		if err := conn.Close(); err != nil {
			errs = append(errs, err)
		}
	}

	p.conns = make(map[string]*Conn)

	if len(errs) > 0 {
		return fmt.Errorf("errors closing connections: %v", errs)
	}

	return nil
}

// IsActive checks if a connection is active
func (c *Conn) IsActive() bool {
	c.mu.RLock()
	defer c.mu.RUnlock()

	if c.ws == nil {
		return false
	}

	// Try to send a ping
	c.ws.SetWriteDeadline(time.Now().Add(1 * time.Second))
	err := c.ws.WriteMessage(websocket.PingMessage, nil)
	return err == nil
}

// Close closes the connection
func (c *Conn) Close() error {
	c.mu.Lock()
	defer c.mu.Unlock()

	if c.ws != nil {
		err := c.ws.Close()
		c.ws = nil
		return err
	}

	return nil
}

// UpdateLastUsed updates the last used timestamp
func (c *Conn) UpdateLastUsed() {
	c.mu.Lock()
	defer c.mu.Unlock()
	c.lastUsed = time.Now()
}

// Send sends a message through the connection
func (c *Conn) Send(msg *RPCMessage) error {
	c.mu.Lock()
	defer c.mu.Unlock()

	if c.ws == nil {
		return fmt.Errorf("connection is closed")
	}

	data, err := msg.Bytes()
	if err != nil {
		return fmt.Errorf("failed to marshal message: %w", err)
	}

	c.ws.SetWriteDeadline(time.Now().Add(10 * time.Second))
	err = c.ws.WriteMessage(websocket.TextMessage, data)
	if err != nil {
		return fmt.Errorf("failed to send message: %w", err)
	}

	c.UpdateLastUsed()
	return nil
}

// Receive receives a message from the connection
func (c *Conn) Receive() (*RPCMessage, error) {
	c.mu.RLock()
	ws := c.ws
	c.mu.RUnlock()

	if ws == nil {
		return nil, fmt.Errorf("connection is closed")
	}

	ws.SetReadDeadline(time.Now().Add(30 * time.Second))
	_, data, err := ws.ReadMessage()
	if err != nil {
		return nil, fmt.Errorf("failed to receive message: %w", err)
	}

	var msg RPCMessage
	if err := json.Unmarshal(data, &msg); err != nil {
		return nil, fmt.Errorf("failed to unmarshal message: %w", err)
	}

	return &msg, nil
}

// GetNodeID returns the node ID of the connection
func (c *Conn) GetNodeID() string {
	c.mu.RLock()
	defer c.mu.RUnlock()
	return c.nodeID
}

// GetAddress returns the address of the connection
func (c *Conn) GetAddress() string {
	c.mu.RLock()
	defer c.mu.RUnlock()
	return c.address
}

// GetLocalAddress returns the local address of the connection
func (c *Conn) GetLocalAddress() net.Addr {
	c.mu.RLock()
	defer c.mu.RUnlock()
	if c.ws != nil {
		return c.ws.LocalAddr()
	}
	return nil
}

// GetRemoteAddress returns the remote address of the connection
func (c *Conn) GetRemoteAddress() net.Addr {
	c.mu.RLock()
	defer c.mu.RUnlock()
	if c.ws != nil {
		return c.ws.RemoteAddr()
	}
	return nil
}
