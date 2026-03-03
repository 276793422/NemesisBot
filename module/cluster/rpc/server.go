// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package rpc

import (
	"fmt"
	"net"
	"sync"
	"time"

	"github.com/276793422/NemesisBot/module/cluster/transport"
)

// Server handles incoming RPC requests
type Server struct {
	cluster   Cluster
	mu        sync.RWMutex
	handlers  map[string]RPCHandler
	running   bool
	listener  net.Listener

	// Configuration
	rpcPort      int
	sendTimeout  time.Duration
	idleTimeout  time.Duration

	// Active connections
	conns      map[string]*transport.TCPConn // remoteAddr -> conn
	connMu     sync.RWMutex
	shutdownCh chan struct{}
}

// RPCHandler is a function that handles an RPC action
type RPCHandler func(payload map[string]interface{}) (map[string]interface{}, error)

// NewServer creates a new RPC server
func NewServer(cluster Cluster) *Server {
	return &Server{
		cluster:       cluster,
		handlers:      make(map[string]RPCHandler),
		conns:         make(map[string]*transport.TCPConn),
		sendTimeout:   10 * time.Second,
		idleTimeout:   60 * time.Second,
		shutdownCh:    make(chan struct{}),
	}
}

// RegisterHandler registers an RPC handler for an action
func (s *Server) RegisterHandler(action string, handler RPCHandler) {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.handlers[action] = handler
}

// Start starts the RPC server on the given port
func (s *Server) Start(port int) error {
	s.mu.Lock()
	if s.running {
		s.mu.Unlock()
		return fmt.Errorf("server already running")
	}
	s.rpcPort = port
	s.mu.Unlock()

	// Register default handlers
	s.registerDefaultHandlers()

	// Create TCP listener
	addr := fmt.Sprintf(":%d", port)
	listener, err := net.Listen("tcp", addr)
	if err != nil {
		return fmt.Errorf("failed to bind port %d: %w", port, err)
	}

	s.mu.Lock()
	s.listener = listener
	s.running = true
	s.mu.Unlock()

	s.cluster.LogRPCInfo("RPC server started on %s", addr)

	// Start accept loop in background
	go s.acceptLoop()

	return nil
}

// Stop stops the RPC server
func (s *Server) Stop() error {
	s.mu.Lock()
	if !s.running {
		s.mu.Unlock()
		return fmt.Errorf("server not running")
	}
	s.running = false
	s.mu.Unlock()

	// Signal shutdown
	close(s.shutdownCh)

	// Close listener
	if s.listener != nil {
		s.listener.Close()
	}

	// Close all connections
	s.connMu.Lock()
	for addr, conn := range s.conns {
		conn.Close()
		s.cluster.LogRPCDebug("Closed connection to %s", addr)
	}
	s.conns = make(map[string]*transport.TCPConn)
	s.connMu.Unlock()

	s.cluster.LogRPCInfo("RPC server stopped")

	return nil
}

// acceptLoop accepts incoming connections
func (s *Server) acceptLoop() {
	for {
		conn, err := s.listener.Accept()
		if err != nil {
			s.mu.RLock()
			running := s.running
			s.mu.RUnlock()

			if !running {
				return // Server stopped
			}

			s.cluster.LogRPCError("Accept error: %v", err)
			continue
		}

		// Handle connection in background
		go s.handleConnection(conn)
	}
}

// handleConnection handles a TCP connection
func (s *Server) handleConnection(netConn net.Conn) {
	remoteAddr := netConn.RemoteAddr().String()

	// Create TCPConn wrapper
	config := &transport.TCPConnConfig{
		NodeID:           "", // Will be set when we know the peer
		Address:          remoteAddr,
		ReadBufferSize:   100,
		SendBufferSize:   100,
		SendTimeout:      s.sendTimeout,
		IdleTimeout:      s.idleTimeout,
		HeartbeatInterval: 0,
	}

	tc := transport.NewTCPConn(netConn, config)
	tc.Start()

	// Add to connections map
	s.connMu.Lock()
	s.conns[remoteAddr] = tc
	s.connMu.Unlock()

	s.cluster.LogRPCInfo("Accepted connection from %s", remoteAddr)

	// Handle messages
	defer func() {
		tc.Close()
		s.connMu.Lock()
		delete(s.conns, remoteAddr)
		s.connMu.Unlock()
		s.cluster.LogRPCDebug("Connection to %s closed", remoteAddr)
	}()

	for {
		select {
		case <-s.shutdownCh:
			return

		case msg, ok := <-tc.Receive():
			if !ok {
				// Connection closed
				return
			}

			if msg == nil {
				continue
			}

			// Handle request
			if msg.Type == transport.RPCTypeRequest {
				s.handleRequest(tc, msg)
			}
		}
	}
}

// handleRequest handles an RPC request
func (s *Server) handleRequest(conn *transport.TCPConn, req *transport.RPCMessage) {
	s.cluster.LogRPCInfo("Received request: action=%s, from=%s, id=%s", req.Action, req.From, req.ID)

	// Update node ID if not set
	if conn.GetNodeID() == "" {
		conn.SetNodeID(req.From)
	}

	// Get handler
	s.mu.RLock()
	handler, exists := s.handlers[req.Action]
	s.mu.RUnlock()

	if !exists {
		// No handler for this action, return default response
		s.cluster.LogRPCInfo("No handler for action '%s', returning default response", req.Action)

		// Create default response
		defaultPayload := map[string]interface{}{
			"response": fmt.Sprintf("Resp: %v", req.Payload),
		}
		resp := transport.NewResponse(req, defaultPayload)
		s.sendMessage(conn, resp)
		return
	}

	// Call handler
	result, err := handler(req.Payload)
	if err != nil {
		s.cluster.LogRPCError("Handler error for action '%s': %v", req.Action, err)
		resp := transport.NewError(req, err.Error())
		s.sendMessage(conn, resp)
		return
	}

	// Send success response
	resp := transport.NewResponse(req, result)
	s.cluster.LogRPCDebug("Sending response: action=%s, id=%s", req.Action, req.ID)
	s.sendMessage(conn, resp)
}

// sendMessage sends a message through the connection
func (s *Server) sendMessage(conn *transport.TCPConn, msg *transport.RPCMessage) error {
	return conn.Send(msg)
}

// registerDefaultHandlers registers default RPC handlers
func (s *Server) registerDefaultHandlers() {
	// Ping handler
	s.RegisterHandler("ping", func(payload map[string]interface{}) (map[string]interface{}, error) {
		return map[string]interface{}{
			"status": "ok",
			"node_id": s.cluster.GetNodeID(),
		}, nil
	})

	// Get capabilities handler
	s.RegisterHandler("get_capabilities", func(payload map[string]interface{}) (map[string]interface{}, error) {
		caps := s.cluster.GetCapabilities()
		return map[string]interface{}{
			"capabilities": caps,
		}, nil
	})

	// Get info handler
	s.RegisterHandler("get_info", func(payload map[string]interface{}) (map[string]interface{}, error) {
		peers := s.cluster.GetOnlinePeers()
		peerInfos := make([]map[string]interface{}, 0, len(peers))
		for _, p := range peers {
			if peer, ok := p.(Node); ok {
				peerInfos = append(peerInfos, map[string]interface{}{
					"id":           peer.GetID(),
					"name":         peer.GetName(),
					"capabilities": peer.GetCapabilities(),
					"status":       peer.GetStatus(),
				})
			}
		}

		return map[string]interface{}{
			"node_id": s.cluster.GetNodeID(),
			"peers":    peerInfos,
		}, nil
	})
}

// GetConnectionCount returns the number of active connections
func (s *Server) GetConnectionCount() int {
	s.connMu.RLock()
	defer s.connMu.RUnlock()
	return len(s.conns)
}

// IsRunning returns true if the server is running
func (s *Server) IsRunning() bool {
	s.mu.RLock()
	defer s.mu.RUnlock()
	return s.running
}
