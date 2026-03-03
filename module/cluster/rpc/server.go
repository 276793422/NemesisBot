// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package rpc

import (
	"encoding/json"
	"fmt"
	"net/http"
	"sync"

	"github.com/gorilla/websocket"
	"github.com/276793422/NemesisBot/module/cluster/transport"
)

// Server handles incoming RPC requests
type Server struct {
	cluster   Cluster
	upgrader  websocket.Upgrader
	mu        sync.RWMutex
	handlers  map[string]RPCHandler
	running   bool
}

// RPCHandler is a function that handles an RPC action
type RPCHandler func(payload map[string]interface{}) (map[string]interface{}, error)

// NewServer creates a new RPC server
func NewServer(cluster Cluster) *Server {
	return &Server{
		cluster: cluster,
		upgrader: websocket.Upgrader{
			ReadBufferSize:  1024,
			WriteBufferSize: 1024,
		},
		handlers: make(map[string]RPCHandler),
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
	s.running = true
	s.mu.Unlock()

	// Register default handlers
	s.registerDefaultHandlers()

	// HTTP handler
	http.HandleFunc("/rpc", s.handleWebSocket)

	// Start HTTP server
	addr := fmt.Sprintf(":%d", port)
	s.cluster.LogRPCInfo("RPC server started on %s", addr)

	go func() {
		if err := http.ListenAndServe(addr, nil); err != nil {
			s.cluster.LogRPCError("RPC server error: %v", err)
		}
	}()

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

	s.cluster.LogRPCInfo("RPC server stopped")

	return nil
}

// handleWebSocket handles WebSocket connections
func (s *Server) handleWebSocket(w http.ResponseWriter, r *http.Request) {
	conn, err := s.upgrader.Upgrade(w, r, nil)
	if err != nil {
		s.cluster.LogRPCError("Failed to upgrade WebSocket: %v", err)
		return
	}

	// Handle connection
	go s.handleConnection(conn)
}

// handleConnection handles a WebSocket connection
func (s *Server) handleConnection(conn *websocket.Conn) {
	defer conn.Close()

	// Message loop
	for {
		_, data, err := conn.ReadMessage()
		if err != nil {
			if err.Error() != "EOF" {
				s.cluster.LogRPCError("Failed to read message: %v", err)
			}
			break
		}

		// Parse message
		var msg transport.RPCMessage
		if err := json.Unmarshal(data, &msg); err != nil {
			s.cluster.LogRPCError("Failed to unmarshal message: %v", err)
			continue
		}

		// Validate message
		if err := msg.Validate(); err != nil {
			s.cluster.LogRPCError("Invalid message: %v", err)
			continue
		}

		// Handle request
		if msg.Type == transport.RPCTypeRequest {
			s.handleRequest(conn, &msg)
		}
	}
}

// handleRequest handles an RPC request
func (s *Server) handleRequest(conn *websocket.Conn, req *transport.RPCMessage) {
	s.cluster.LogRPCInfo("Received request: action=%s, from=%s, id=%s", req.Action, req.From, req.ID)

	// Get handler
	s.mu.RLock()
	handler, exists := s.handlers[req.Action]
	s.mu.RUnlock()

	if !exists {
		// No handler for this action, return default response
		s.cluster.LogRPCInfo("No handler for action '%s', returning default response", req.Action)

		// Create default response: Resp: + payload
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
	s.cluster.LogRPCInfo("Sending response: action=%s, id=%s", req.Action, req.ID)
	s.sendMessage(conn, resp)
}

// sendMessage sends a message through WebSocket
func (s *Server) sendMessage(conn *websocket.Conn, msg *transport.RPCMessage) error {
	data, err := msg.Bytes()
	if err != nil {
		return fmt.Errorf("failed to marshal message: %w", err)
	}

	err = conn.WriteMessage(websocket.TextMessage, data)
	if err != nil {
		return fmt.Errorf("failed to send message: %w", err)
	}

	s.cluster.LogRPCDebug("Sent message: type=%s, id=%s", msg.Type, msg.ID)

	return nil
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
