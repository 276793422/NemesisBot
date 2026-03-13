// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
// Web Channel Module - HTTP Server

package web

import (
	"context"
	"fmt"
	"net/http"
	"path/filepath"
	"sync"
	"time"

	"github.com/276793422/NemesisBot/module/bus"
	"github.com/276793422/NemesisBot/module/logger"
	"github.com/gorilla/websocket"
)

// Server represents the HTTP server for the web channel
type Server struct {
	host        string
	port        int
	wsPath      string
	authToken   string
	sessionMgr  *SessionManager
	httpServer  *http.Server
	running     bool
	mu          sync.RWMutex
	corsManager *CORSManager // CORS manager

	// Channel for incoming messages from WebSocket clients
	messageChan chan IncomingMessage
	// Bus for publishing inbound messages
	bus *bus.MessageBus
}

// ServerConfig holds the server configuration
type ServerConfig struct {
	Host        string
	Port        int
	WSPath      string
	AuthToken   string
	SessionMgr  *SessionManager
	Bus         *bus.MessageBus
	Workspace   string // Workspace path for config files
}

// NewServer creates a new HTTP server
func NewServer(config ServerConfig) *Server {
	s := &Server{
		host:       config.Host,
		port:       config.Port,
		wsPath:     config.WSPath,
		authToken:  config.AuthToken,
		sessionMgr: config.SessionMgr,
		bus:        config.Bus,
		running:    false,
	}

	// Initialize CORS manager if workspace is provided
	if config.Workspace != "" {
		corsConfigPath := filepath.Join(config.Workspace, "config", "cors.json")
		corsMgr, err := NewCORSManager(corsConfigPath)
		if err != nil {
			logger.WarnCF("web", "Failed to initialize CORS manager, CORS checks will be disabled", map[string]interface{}{
				"error": err.Error(),
			})
			// Continue without CORS manager (CORS checks will be disabled)
		} else {
			s.corsManager = corsMgr
			logger.InfoCF("web", "CORS manager initialized", map[string]interface{}{
				"config_path": corsConfigPath,
			})
		}
	}

	// Start outbound message dispatcher
	// DISABLED: Now using unified dispatcher from channels.Manager
	// Web server should NOT read from outbound channel directly
	// go s.dispatchOutbound()

	return s
}

// Start starts the HTTP server
func (s *Server) Start(ctx context.Context) error {
	s.mu.Lock()
	if s.running {
		s.mu.Unlock()
		return fmt.Errorf("server already running")
	}
	s.running = true
	s.mu.Unlock()

	// Create message channel for WebSocket handlers
	s.messageChan = make(chan IncomingMessage, 100)

	// Start message processor
	go s.processMessages(ctx)

	// Create mux
	mux := http.NewServeMux()

	// Serve static files (embedded)
	mux.HandleFunc("/", s.handleStaticFile)
	mux.HandleFunc("/style.css", s.handleStaticFile)
	mux.HandleFunc("/app.js", s.handleStaticFile)

	// WebSocket endpoint
	mux.HandleFunc(s.wsPath, s.handleWebSocket)

	// Health check endpoint
	mux.HandleFunc("/health", s.handleHealth)

	// Create HTTP server
	s.httpServer = &http.Server{
		Addr:         fmt.Sprintf("%s:%d", s.host, s.port),
		Handler:      mux,
		ReadTimeout:  10 * time.Second,
		WriteTimeout: 10 * time.Second,
		IdleTimeout:  60 * time.Second,
	}

	// Start server in goroutine
	errChan := make(chan error, 1)
	go func() {
		logger.InfoCF("web", "HTTP server starting", map[string]interface{}{
			"address": fmt.Sprintf("%s:%d", s.host, s.port),
		})
		if err := s.httpServer.ListenAndServe(); err != nil && err != http.ErrServerClosed {
			errChan <- err
		}
	}()

	// Wait for context cancellation or error
	select {
	case <-ctx.Done():
		logger.InfoC("web", "HTTP server shutdown requested")
		return s.Shutdown(ctx)
	case err := <-errChan:
		return fmt.Errorf("HTTP server error: %w", err)
	}
}

// Stop stops the HTTP server
func (s *Server) Stop(ctx context.Context) error {
	return s.Shutdown(ctx)
}

// Shutdown gracefully shuts down the server
func (s *Server) Shutdown(ctx context.Context) error {
	s.mu.Lock()
	if !s.running {
		s.mu.Unlock()
		return nil
	}
	s.running = false
	s.mu.Unlock()

	logger.InfoC("web", "Shutting down HTTP server")

	// Create shutdown context with timeout
	shutdownCtx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()

	// Shutdown HTTP server
	if err := s.httpServer.Shutdown(shutdownCtx); err != nil {
		logger.ErrorCF("web", "HTTP server shutdown error", map[string]interface{}{
			"error": err.Error(),
		})
		return err
	}

	logger.InfoC("web", "HTTP server stopped")
	return nil
}

// IsRunning returns whether the server is running
func (s *Server) IsRunning() bool {
	s.mu.RLock()
	defer s.mu.RUnlock()
	return s.running
}

// handleStaticFile serves static files
func (s *Server) handleStaticFile(w http.ResponseWriter, r *http.Request) {
	// Get file from embedded filesystem
	staticFS, err := StaticFiles()
	if err != nil {
		http.Error(w, "Static files not available", http.StatusInternalServerError)
		return
	}
	path := r.URL.Path[1:] // Remove leading slash
	if path == "" {
		path = "index.html"
	}

	file, err := staticFS.Open(path)
	if err != nil {
		http.Error(w, "File not found", http.StatusNotFound)
		return
	}
	defer file.Close()

	// Get file info
	stat, err := file.Stat()
	if err != nil {
		http.Error(w, "File error", http.StatusInternalServerError)
		return
	}

	// Set content type
	contentType := "text/html; charset=utf-8"
	switch path {
	case "style.css":
		contentType = "text/css; charset=utf-8"
	case "app.js":
		contentType = "application/javascript; charset=utf-8"
	}
	w.Header().Set("Content-Type", contentType)

	// Read file content
	content := make([]byte, stat.Size())
	_, err = file.Read(content)
	if err != nil {
		http.Error(w, "Read error", http.StatusInternalServerError)
		return
	}

	// Write to response
	w.Write(content)
}

// handleWebSocket handles WebSocket upgrade requests
func (s *Server) handleWebSocket(w http.ResponseWriter, r *http.Request) {
	// CORS check (if CORS manager is configured)
	if s.corsManager != nil {
		if !s.corsManager.CheckCORS(r) {
			logger.WarnCF("web", "CORS violation blocked", map[string]interface{}{
				"origin":      r.Header.Get("Origin"),
				"remote_addr": r.RemoteAddr,
			})
			http.Error(w, "Origin not allowed", http.StatusForbidden)
			return
		}
	}

	// Check auth token if configured
	if s.authToken != "" {
		token := r.URL.Query().Get("token")
		if token != s.authToken {
			logger.WarnCF("web", "WebSocket authentication failed", map[string]interface{}{
				"remote_addr": r.RemoteAddr,
			})
			http.Error(w, "Unauthorized", http.StatusUnauthorized)
			return
		}
	}

	// Upgrade to WebSocket
	conn, err := websocketUpgrader.Upgrade(w, r, nil)
	if err != nil {
		logger.ErrorCF("web", "WebSocket upgrade failed", map[string]interface{}{
			"error":       err.Error(),
			"remote_addr": r.RemoteAddr,
		})
		return
	}

	// Create session
	session := s.sessionMgr.CreateSession(conn)

	logger.InfoCF("web", "WebSocket connection established", map[string]interface{}{
		"session_id":  session.ID,
		"remote_addr": r.RemoteAddr,
		"origin":      r.Header.Get("Origin"),
	})

	// Handle WebSocket connection in goroutine
	go func() {
		defer func() {
			// Clean up session on disconnect
			_ = conn.Close()
			s.sessionMgr.RemoveSession(session.ID)
			logger.InfoCF("web", "WebSocket connection closed", map[string]interface{}{
				"session_id": session.ID,
			})
		}()

		if err := HandleWebSocket(session, s.sessionMgr, s.messageChan, s.authToken); err != nil {
			logger.ErrorCF("web", "WebSocket handler error", map[string]interface{}{
				"error":      err.Error(),
				"session_id": session.ID,
			})
		}
	}()
}

// handleHealth handles health check requests
func (s *Server) handleHealth(w http.ResponseWriter, r *http.Request) {
	stats := s.sessionMgr.Stats()

	w.Header().Set("Content-Type", "application/json")
	_, _ = fmt.Fprintf(w, `{"status":"ok","running":%t,"sessions":%d}`, s.running, stats["active_sessions"])
}

// processMessages processes incoming messages from WebSocket clients
func (s *Server) processMessages(ctx context.Context) {
	for {
		select {
		case <-ctx.Done():
			logger.DebugC("web", "Message processor stopped")
			return
		case msg := <-s.messageChan:
			// Publish to message bus
			inboundMsg := bus.InboundMessage{
				Channel:  "web",
				SenderID: msg.SenderID,
				ChatID:   msg.ChatID,
				Content:  msg.Content,
				Media:    []string{},
			}
			s.bus.PublishInbound(inboundMsg)

			logger.DebugCF("web", "Message published to bus", map[string]interface{}{
				"session_id": msg.SessionID,
				"sender_id":  msg.SenderID,
				"chat_id":    msg.ChatID,
			})
		}
	}
}

// SendToSession sends a message to a specific session
func (s *Server) SendToSession(sessionID, role, content string) error {
	return BroadcastToSession(s.sessionMgr, sessionID, role, content)
}

// GetSessionManager returns the session manager
func (s *Server) GetSessionManager() *SessionManager {
	return s.sessionMgr
}

// dispatchOutbound handles outbound messages from the message bus
func (s *Server) dispatchOutbound() {
	for {
		// Subscribe to outbound messages with a context that never cancels
		ctx := context.Background()
		msg, ok := s.bus.SubscribeOutbound(ctx)
		if !ok {
			continue
		}

		// Only handle messages for the web channel
		if msg.Channel != "web" {
			continue
		}

		// Extract session ID from chat ID
		var sessionID string
		if len(msg.ChatID) > 4 && msg.ChatID[:4] == "web:" {
			sessionID = msg.ChatID[4:]
		} else {
			logger.WarnCF("web", "Invalid chat ID format", map[string]interface{}{
				"chat_id": msg.ChatID,
			})
			continue
		}

		// Send message to session
		if err := s.SendToSession(sessionID, "assistant", msg.Content); err != nil {
			logger.ErrorCF("web", "Failed to send outbound message", map[string]interface{}{
				"error":      err.Error(),
				"session_id": sessionID,
			})
		}
	}
}

// Custom WebSocket upgrader to avoid import issues
var websocketUpgrader = &websocket.Upgrader{
	ReadBufferSize:  1024,
	WriteBufferSize: 1024,
	CheckOrigin: func(r *http.Request) bool {
		return true // Allow all origins
	},
}
