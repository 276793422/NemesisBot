// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package http

import (
	"net"
	"net/http"
	"time"
)

// PooledTransport wraps a shared http.Transport and http.Client.
// In Go, the standard approach for HTTP connection pooling is to share a single
// http.Client with a properly configured http.Transport, which internally manages
// connection reuse, keep-alive, and concurrency via sync.WaitGroup and connection pools.
//
// This is more efficient than pooling multiple http.Client instances because:
//   - A single Transport manages a pool of TCP connections per host
//   - Connections are reused across requests (HTTP keep-alive)
//   - Thread-safe by design (Transport internals use proper synchronization)
type PooledTransport struct {
	transport *http.Transport
	client    *http.Client
}

// NewPooledTransport creates a new PooledTransport with the given connection pool settings.
//
// Parameters:
//   - maxIdleConnsPerHost: maximum number of idle connections per host
//   - maxIdleConns: total maximum idle connections across all hosts
//   - idleConnTimeout: how long idle connections remain in the pool before closing
func NewPooledTransport(maxIdleConnsPerHost int, maxIdleConns int, idleConnTimeout time.Duration) *PooledTransport {
	transport := &http.Transport{
		DialContext: (&net.Dialer{
			Timeout:   30 * time.Second,
			KeepAlive: 30 * time.Second,
		}).DialContext,
		MaxIdleConns:        maxIdleConns,
		MaxIdleConnsPerHost: maxIdleConnsPerHost,
		IdleConnTimeout:     idleConnTimeout,
		TLSHandshakeTimeout: 10 * time.Second,
		ForceAttemptHTTP2:   true,
	}

	client := &http.Client{
		Transport: transport,
		Timeout:   60 * time.Second,
	}

	return &PooledTransport{
		transport: transport,
		client:    client,
	}
}

// Client returns the shared HTTP client backed by the pooled transport.
func (pt *PooledTransport) Client() *http.Client {
	return pt.client
}

// Transport returns the underlying http.Transport for advanced configuration.
func (pt *PooledTransport) Transport() *http.Transport {
	return pt.transport
}

// Close closes all idle connections in the transport pool.
// This should be called during shutdown to release resources.
func (pt *PooledTransport) Close() {
	pt.transport.CloseIdleConnections()
}

// DefaultPooledTransport creates a PooledTransport with sensible defaults
// suitable for most bot-to-API communication patterns:
//   - 10 idle connections per host (enough for concurrent API calls)
//   - 100 total idle connections (multiple API providers)
//   - 90 second idle timeout (longer than typical server keep-alive)
func DefaultPooledTransport() *PooledTransport {
	return NewPooledTransport(10, 100, 90*time.Second)
}
