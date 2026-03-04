// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package transport

import (
	"context"
	"fmt"
	"net"
	"sync"
	"time"
)

const (
	// DefaultMaxConns is the default maximum number of concurrent connections
	DefaultMaxConns = 50
	// DefaultMaxConnsPerNode is the default maximum connections per node
	DefaultMaxConnsPerNode = 3
)

// Pool manages a pool of TCP connections
type Pool struct {
	mu        sync.RWMutex
	conns     map[string]*TCPConn // node_id:address -> TCPConn
	timeout   time.Duration

	// Connection limits
	maxConns           int // Maximum total concurrent connections
	maxConnsPerNode    int // Maximum concurrent connections per node
	semaphore          chan struct{} // Semaphore for limiting total connections
	activeConns        int // Counter for active connections (must hold mu to access)

	// Per-node connection counters (must hold mu to access)
	nodeConns map[string]int // node_id -> active connection count

	// Configuration
	dialTimeout   time.Duration
	idleTimeout   time.Duration
	sendTimeout   time.Duration
}

// PoolConfig contains configuration for creating a new Pool
type PoolConfig struct {
	MaxConns        int
	MaxConnsPerNode int
	DialTimeout     time.Duration
	IdleTimeout     time.Duration
	SendTimeout     time.Duration
}

// DefaultPoolConfig returns the default pool configuration
func DefaultPoolConfig() *PoolConfig {
	return &PoolConfig{
		MaxConns:        DefaultMaxConns,
		MaxConnsPerNode: DefaultMaxConnsPerNode,
		DialTimeout:     10 * time.Second,
		IdleTimeout:     30 * time.Second,
		SendTimeout:     10 * time.Second,
	}
}

// NewPool creates a new connection pool
func NewPool() *Pool {
	return NewPoolWithConfig(DefaultPoolConfig())
}

// NewPoolWithConfig creates a new connection pool with custom configuration
func NewPoolWithConfig(config *PoolConfig) *Pool {
	return &Pool{
		conns:            make(map[string]*TCPConn),
		timeout:           10 * time.Second,
		maxConns:          config.MaxConns,
		maxConnsPerNode:   config.MaxConnsPerNode,
		semaphore:         make(chan struct{}, config.MaxConns),
		nodeConns:         make(map[string]int),
		dialTimeout:       config.DialTimeout,
		idleTimeout:       config.IdleTimeout,
		sendTimeout:       config.SendTimeout,
	}
}

// Get gets or creates a connection to a node
// If ctx is provided, it can be used to cancel the dial operation
func (p *Pool) Get(nodeID, address string) (*TCPConn, error) {
	return p.GetWithContext(context.Background(), nodeID, address)
}

// GetWithContext gets or creates a connection to a node with context support
func (p *Pool) GetWithContext(ctx context.Context, nodeID, address string) (*TCPConn, error) {
	// Use nodeID:address as the cache key to support multiple addresses per node
	key := nodeID + ":" + address

	p.mu.RLock()
	conn, exists := p.conns[key]
	if exists && conn.IsActive() {
		p.mu.RUnlock()
		conn.UpdateLastUsed()
		return conn, nil
	}
	p.mu.RUnlock()

	// Acquire semaphore slot (with context timeout to avoid blocking forever)
	select {
	case p.semaphore <- struct{}{}:
		// Acquired slot
	case <-time.After(5 * time.Second):
		return nil, fmt.Errorf("connection pool exhausted (max=%d)", p.maxConns)
	case <-ctx.Done():
		return nil, fmt.Errorf("connection cancelled: %w", ctx.Err())
	}

	// Create new connection
	p.mu.Lock()
	defer p.mu.Unlock()

	// Double-check after acquiring write lock
	if conn, exists := p.conns[key]; exists && conn.IsActive() {
		// Connection was created while we were waiting for write lock
		// Release the semaphore slot we acquired
		<-p.semaphore
		conn.UpdateLastUsed()
		return conn, nil
	}

	// ✅ Fix: Clean up inactive connection before checking limit
	if exists && !conn.IsActive() {
		// Connection exists but is inactive (closed by idle monitor or error)
		// Remove it from pool and decrement counters
		delete(p.conns, key)
		p.activeConns--
		if p.nodeConns[nodeID] > 0 {
			p.nodeConns[nodeID]--
		}
	}

	// Check per-node limit
	if p.nodeConns[nodeID] >= p.maxConnsPerNode {
		<-p.semaphore // Release semaphore slot
		return nil, fmt.Errorf("too many concurrent connections to node %s (max=%d)", nodeID, p.maxConnsPerNode)
	}

	// Dial TCP connection
	tcpConn, err := p.dial(ctx, nodeID, address)
	if err != nil {
		<-p.semaphore // Release semaphore slot on failure
		return nil, fmt.Errorf("failed to dial %s: %w", address, err)
	}

	// Add to pool
	p.conns[key] = tcpConn
	p.activeConns++
	p.nodeConns[nodeID]++

	return tcpConn, nil
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
				p.activeConns--
			}
		}
		// Reset node counter
		delete(p.nodeConns, nodeID)
	} else {
		// Remove specific connection
		key := nodeID + ":" + address
		if conn, ok := p.conns[key]; ok {
			conn.Close()
			delete(p.conns, key)
			p.activeConns--

			// Decrement node counter and release semaphore
			if p.nodeConns[nodeID] > 0 {
				p.nodeConns[nodeID]--
			}
		}
	}

	// Release semaphore slot
	select {
	case <-p.semaphore:
		// Successfully released
	default:
		// Semaphore was full, nothing to release
	}
}

// dial creates a new TCP connection
func (p *Pool) dial(ctx context.Context, nodeID, address string) (*TCPConn, error) {
	// Create dialer with timeout
	dialer := net.Dialer{
		Timeout: p.dialTimeout,
	}

	// Dial TCP connection
	conn, err := dialer.DialContext(ctx, "tcp", address)
	if err != nil {
		return nil, err
	}

	// Create TCPConn wrapper
	config := &TCPConnConfig{
		NodeID:           nodeID,
		Address:          address,
		ReadBufferSize:   100,
		SendBufferSize:   100,
		SendTimeout:      p.sendTimeout,
		IdleTimeout:      p.idleTimeout,
		HeartbeatInterval: 0,
	}

	tcpConn := NewTCPConn(conn, config)
	tcpConn.Start()

	return tcpConn, nil
}

// Close closes all connections in the pool
func (p *Pool) Close() error {
	p.mu.Lock()
	defer p.mu.Unlock()

	var errs []error

	// Close all connections
	for _, conn := range p.conns {
		if err := conn.Close(); err != nil {
			errs = append(errs, err)
		}
	}

	p.conns = make(map[string]*TCPConn)
	p.nodeConns = make(map[string]int)
	p.activeConns = 0

	// Drain semaphore to capacity (release all slots)
	for i := 0; i < cap(p.semaphore); i++ {
		select {
		case <-p.semaphore:
			// Successfully released a slot
		default:
			// No more slots to release
		}
	}

	if len(errs) > 0 {
		return fmt.Errorf("errors closing connections: %v", errs)
	}

	return nil
}

// GetStats returns statistics about the connection pool
func (p *Pool) GetStats() PoolStats {
	p.mu.RLock()
	defer p.mu.RUnlock()

	stats := PoolStats{
		ActiveConns:   p.activeConns,
		MaxConns:      p.maxConns,
		AvailableSlots: cap(p.semaphore) - len(p.semaphore),
	}

	// Count connections per node
	stats.NodeConns = make(map[string]int, len(p.nodeConns))
	for node, count := range p.nodeConns {
		stats.NodeConns[node] = count
	}

	return stats
}

// PoolStats contains statistics about the connection pool
type PoolStats struct {
	ActiveConns    int            // Number of active connections
	MaxConns       int            // Maximum allowed connections
	AvailableSlots int            // Available semaphore slots
	NodeConns      map[string]int // Connections per node
}
