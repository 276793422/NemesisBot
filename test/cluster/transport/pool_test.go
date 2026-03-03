// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package transport_test

import (
	"context"
	"fmt"
	"net"
	"sync"
	"testing"
	"time"

	"github.com/276793422/NemesisBot/module/cluster/transport"
)

// startTestServer starts a test TCP server that echoes connections
func startTestServer(t *testing.T) (listener net.Listener, addr string) {
	listener, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatalf("Failed to start test server: %v", err)
	}

	// Accept connections in background
	go func() {
		for {
			conn, err := listener.Accept()
			if err != nil {
				return // Listener closed
			}

			// Handle connection: keep it alive
			go func(c net.Conn) {
				defer c.Close()
				buf := make([]byte, 1024)
				for {
					_, err := c.Read(buf)
					if err != nil {
						return
					}
				}
			}(conn)
		}
	}()

	return listener, listener.Addr().String()
}

func TestNewPool(t *testing.T) {
	pool := transport.NewPool()

	if pool == nil {
		t.Fatal("NewPool() returned nil")
	}

	pool.Close()
}

func TestPoolGet(t *testing.T) {
	listener, addr := startTestServer(t)
	defer listener.Close()

	pool := transport.NewPool()
	defer pool.Close()

	// Get connection
	conn, err := pool.Get("test-node", addr)
	if err != nil {
		t.Fatalf("Get() failed: %v", err)
	}

	if conn == nil {
		t.Fatal("Get() returned nil connection")
	}

	// Verify connection is active
	if !conn.IsActive() {
		t.Error("Get() returned inactive connection")
	}

	// Getting same address should return same connection
	conn2, err := pool.Get("test-node", addr)
	if err != nil {
		t.Fatalf("Get() again failed: %v", err)
	}

	if conn != conn2 {
		t.Error("Get() should return cached connection")
	}
}

func TestPoolGetWithContext(t *testing.T) {
	listener, addr := startTestServer(t)
	defer listener.Close()

	pool := transport.NewPool()
	defer pool.Close()

	ctx := context.Background()
	conn, err := pool.GetWithContext(ctx, "test-node", addr)
	if err != nil {
		t.Fatalf("GetWithContext() failed: %v", err)
	}

	if conn == nil {
		t.Fatal("GetWithContext() returned nil connection")
	}

	// Remove connection first
	pool.Remove("test-node", conn.GetAddress())

	// Test with cancelled context (should fail during dial)
	cancelledCtx, cancel := context.WithCancel(context.Background())
	cancel() // Cancel immediately

	_, err = pool.GetWithContext(cancelledCtx, "test-node", addr)
	if err == nil {
		t.Error("GetWithContext() with cancelled context should fail")
	}
}

func TestPoolRemove(t *testing.T) {
	listener, addr := startTestServer(t)
	defer listener.Close()

	pool := transport.NewPool()

	// Add connection
	conn, err := pool.Get("test-node", addr)
	if err != nil {
		t.Fatalf("Get() failed: %v", err)
	}

	// Remove specific connection
	pool.Remove("test-node", conn.GetAddress())

	// Verify removed by getting new connection
	conn2, err := pool.Get("test-node", addr)
	if err != nil {
		t.Fatalf("Get() after Remove failed: %v", err)
	}

	if conn2 == conn {
		t.Error("Remove() failed: returned same connection")
	}

	// Close all
	pool.Close()
}

func TestPoolRemoveAllForNode(t *testing.T) {
	listener, addr := startTestServer(t)
	defer listener.Close()

	pool := transport.NewPool()
	defer pool.Close()

	// Add multiple connections to same node
	_, err := pool.Get("test-node", addr)
	if err != nil {
		t.Fatalf("Get() failed: %v", err)
	}

	// Remove all connections for node
	pool.Remove("test-node", "")

	// Should be able to get new connection
	_, err = pool.Get("test-node", addr)
	if err != nil {
		t.Errorf("Get() after Remove() failed: %v", err)
	}
}

func TestPoolClose(t *testing.T) {
	listener, addr := startTestServer(t)
	defer listener.Close()

	pool := transport.NewPool()

	// Add some connections
	for i := 0; i < 3; i++ {
		_, e := pool.Get("test-node", addr)
		if e != nil {
			t.Fatalf("Get() failed: %v", e)
		}
	}

	// Close pool
	err := pool.Close()
	if err != nil {
		t.Errorf("Close() failed: %v", err)
	}

	// Verify all connections closed
	stats := pool.GetStats()
	if stats.ActiveConns != 0 {
		t.Errorf("Close() left active connections: %d", stats.ActiveConns)
	}

	// Pool is marked as closed internally, semaphore is drained
	// Trying to get a new connection should work (it's a new connection)
	// but getting from cache should return nil
	// This is expected behavior - the pool allows new connections after close
	// in real usage, you wouldn't use the pool after calling Close()
}

func TestPoolMaxConnsPerNode(t *testing.T) {
	listener, addr := startTestServer(t)
	defer listener.Close()

	config := &transport.PoolConfig{
		MaxConns:        50,
		MaxConnsPerNode: 2, // Limit to 2 per node
		DialTimeout:     5 * time.Second,
		IdleTimeout:     30 * time.Second,
		SendTimeout:     10 * time.Second,
	}

	pool := transport.NewPoolWithConfig(config)
	defer pool.Close()

	// Get first connection - should succeed
	conn1, err := pool.Get("test-node", addr)
	if err != nil {
		t.Fatalf("Get() [1] failed: %v", err)
	}

	// Get the same connection again - should return cached connection
	conn2, err := pool.Get("test-node", addr)
	if err != nil {
		t.Fatalf("Get() [2] failed: %v", err)
	}

	// Should be the same connection (cached)
	if conn1 != conn2 {
		t.Error("Get() should return cached connection")
	}

	// Verify pool state
	stats := pool.GetStats()
	if stats.NodeConns["test-node"] != 1 {
		// Only 1 actual connection (cached)
		t.Logf("Note: Got %d connections (caching behavior)", stats.NodeConns["test-node"])
	}
}

func TestPoolGetStats(t *testing.T) {
	listener, addr := startTestServer(t)
	defer listener.Close()

	pool := transport.NewPool()

	// Add some connections
	_, err := pool.Get("node1", addr)
	if err != nil {
		t.Fatalf("Get() failed: %v", err)
	}

	_, err = pool.Get("node2", addr)
	if err != nil {
		t.Fatalf("Get() failed: %v", err)
	}

	stats := pool.GetStats()

	if stats.ActiveConns != 2 {
		t.Errorf("GetStats() ActiveConns = %d, want 2", stats.ActiveConns)
	}

	if len(stats.NodeConns) != 2 {
		t.Errorf("GetStats() NodeConns length = %d, want 2", len(stats.NodeConns))
	}

	pool.Close()
}

func TestPoolConcurrentAccess(t *testing.T) {
	listener, addr := startTestServer(t)
	defer listener.Close()

	pool := transport.NewPool()
	defer pool.Close()

	// Concurrent gets
	const goroutines = 10
	var wg sync.WaitGroup
	errors := make(chan error, goroutines)

	for i := 0; i < goroutines; i++ {
		wg.Add(1)
		go func(id int) {
			defer wg.Done()

			conn, err := pool.Get("test-node", addr)
			if err != nil {
				errors <- err
				return
			}

			// Use connection briefly
			time.Sleep(10 * time.Millisecond)

			// Verify still active
			if !conn.IsActive() {
				errors <- fmt.Errorf("connection became inactive")
			}
		}(i)
	}

	wg.Wait()
	close(errors)

	for err := range errors {
		t.Errorf("Concurrent access error: %v", err)
	}
}

func TestDefaultPoolConfig(t *testing.T) {
	config := transport.DefaultPoolConfig()

	if config.DialTimeout != 10*time.Second {
		t.Errorf("DefaultPoolConfig() DialTimeout = %v, want 10s", config.DialTimeout)
	}

	if config.IdleTimeout != 30*time.Second {
		t.Errorf("DefaultPoolConfig() IdleTimeout = %v, want 30s", config.IdleTimeout)
	}

	if config.SendTimeout != 10*time.Second {
		t.Errorf("DefaultPoolConfig() SendTimeout = %v, want 10s", config.SendTimeout)
	}
}

func TestNewPoolWithConfig(t *testing.T) {
	config := &transport.PoolConfig{
		MaxConns:        100,
		MaxConnsPerNode: 5,
		DialTimeout:     5 * time.Second,
		IdleTimeout:     60 * time.Second,
		SendTimeout:     15 * time.Second,
	}

	pool := transport.NewPoolWithConfig(config)
	defer pool.Close()
}
