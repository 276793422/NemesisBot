// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
// WebSocket Channel - Standalone WebSocket channel for external program integration

package channels

import (
	"context"
	"encoding/json"
	"fmt"
	"net/http"
	"sync"
	"sync/atomic"
	"time"

	"github.com/gorilla/websocket"
	"github.com/276793422/NemesisBot/module/bus"
	"github.com/276793422/NemesisBot/module/config"
	"github.com/276793422/NemesisBot/module/logger"
)

// WebSocketChannel manages a standalone WebSocket server for external program integration
// Only allows one client connection at a time
type WebSocketChannel struct {
	*BaseChannel
	config       *config.WebSocketChannelConfig
	server       *http.Server
	running      atomic.Bool
	stopped      chan struct{}
	connMu       sync.RWMutex
	conn         *websocket.Conn
	clientID     string
	wg           sync.WaitGroup
}

// Message types for WebSocket communication
const (
	MessageTypeMessage = "message"
	MessageTypePing    = "ping"
	MessageTypePong    = "pong"
	MessageTypeError   = "error"
)

// ClientMessage is a message sent from the client to the server
type ClientMessage struct {
	Type      string    `json:"type"`
	Content   string    `json:"content,omitempty"`
	Timestamp time.Time `json:"timestamp"`
}

// ServerMessage is a message sent from the server to the client
type ServerMessage struct {
	Type      string    `json:"type"`
	Role      string    `json:"role,omitempty"`      // "user" or "assistant"
	Content   string    `json:"content,omitempty"`
	Timestamp time.Time `json:"timestamp"`
	Error     string    `json:"error,omitempty"`
}

// NewWebSocketChannel creates a new WebSocket channel
func NewWebSocketChannel(cfg *config.WebSocketChannelConfig, messageBus *bus.MessageBus) (*WebSocketChannel, error) {
	base := NewBaseChannel("websocket", cfg, messageBus, cfg.AllowFrom)

	return &WebSocketChannel{
		BaseChannel: base,
		config:      cfg,
		stopped:     make(chan struct{}),
	}, nil
}

// Start starts the WebSocket server
func (c *WebSocketChannel) Start(ctx context.Context) error {
	logger.InfoCF("websocket", "Starting WebSocket channel", map[string]interface{}{
		"host":       c.config.Host,
		"port":       c.config.Port,
		"path":       c.config.Path,
		"sync_to_web": c.config.SyncToWeb,
	})

	// Create HTTP server
	mux := http.NewServeMux()
	mux.HandleFunc(c.config.Path, c.handleWebSocket)

	c.server = &http.Server{
		Addr:         fmt.Sprintf("%s:%d", c.config.Host, c.config.Port),
		Handler:      mux,
		ReadTimeout:  10 * time.Second,
		WriteTimeout: 10 * time.Second,
		IdleTimeout:  365 * 24 * time.Hour, // 1 year - effectively no timeout
	}

	// Start server in goroutine
	errChan := make(chan error, 1)
	go func() {
		logger.InfoCF("websocket", "WebSocket server listening", map[string]interface{}{
			"address": c.server.Addr,
		})
		if err := c.server.ListenAndServe(); err != nil && err != http.ErrServerClosed {
			errChan <- err
		}
	}()

	c.running.Store(true)

	// Wait briefly to ensure server starts successfully
	select {
	case err := <-errChan:
		c.running.Store(false)
		return err
	case <-time.After(100 * time.Millisecond):
		logger.InfoC("websocket", "WebSocket channel started successfully")
		return nil
	}
}

// Stop stops the WebSocket server
func (c *WebSocketChannel) Stop(ctx context.Context) error {
	logger.InfoC("websocket", "Stopping WebSocket channel")

	if !c.running.Load() {
		return nil
	}

	c.running.Store(false)
	close(c.stopped)

	// Close client connection if exists
	c.connMu.Lock()
	if c.conn != nil {
		_ = c.conn.Close()
		c.conn = nil
	}
	c.connMu.Unlock()

	// Shutdown server
	if c.server != nil {
		shutdownCtx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
		defer cancel()
		if err := c.server.Shutdown(shutdownCtx); err != nil {
			logger.ErrorCF("websocket", "Error shutting down server", map[string]interface{}{
				"error": err.Error(),
			})
			return err
		}
	}

	// Wait for all goroutines to finish
	c.wg.Wait()

	logger.InfoC("websocket", "WebSocket channel stopped")
	return nil
}

// IsRunning returns whether the channel is running
func (c *WebSocketChannel) IsRunning() bool {
	return c.running.Load()
}

// Send sends a message to the WebSocket client
func (c *WebSocketChannel) Send(ctx context.Context, msg bus.OutboundMessage) error {
	if !c.running.Load() {
		return fmt.Errorf("websocket channel not running")
	}

	c.connMu.RLock()
	conn := c.conn
	c.connMu.RUnlock()

	if conn == nil {
		return fmt.Errorf("no client connected")
	}

	logger.DebugCF("websocket", "Sending message to client", map[string]interface{}{
		"content": msg.Content,
	})

	// Send message to client
	serverMsg := ServerMessage{
		Type:      MessageTypeMessage,
		Role:      "assistant",
		Content:   msg.Content,
		Timestamp: time.Now(),
	}

	data, err := json.Marshal(serverMsg)
	if err != nil {
		return fmt.Errorf("failed to marshal message: %w", err)
	}

	// Set write deadline
	if err := conn.SetWriteDeadline(time.Now().Add(10 * time.Second)); err != nil {
		return fmt.Errorf("failed to set write deadline: %w", err)
	}

	// Send message
	if err := conn.WriteMessage(websocket.TextMessage, data); err != nil {
		return fmt.Errorf("failed to send message: %w", err)
	}

	// Sync to configured targets if enabled
	if c.config.SyncToWeb || len(c.config.SyncTo) > 0 {
		c.SyncToTargets("assistant", msg.Content)
	}

	return nil
}

// handleWebSocket handles WebSocket connection requests
func (c *WebSocketChannel) handleWebSocket(w http.ResponseWriter, r *http.Request) {
	// Check if a client is already connected
	c.connMu.Lock()
	if c.conn != nil {
		c.connMu.Unlock()
		logger.WarnC("websocket", "Rejected connection: client already connected")
		http.Error(w, "Another client is already connected", http.StatusServiceUnavailable)
		return
	}
	c.connMu.Unlock()

	// Check auth token if configured (optional)
	if c.config.AuthToken != "" {
		token := r.URL.Query().Get("token")
		if token != c.config.AuthToken {
			logger.WarnCF("websocket", "WebSocket authentication failed", map[string]interface{}{
				"remote_addr": r.RemoteAddr,
			})
			http.Error(w, "Unauthorized", http.StatusUnauthorized)
			return
		}
	}

	// Upgrade to WebSocket
	upgrader := &websocket.Upgrader{
		ReadBufferSize:  1024,
		WriteBufferSize: 1024,
		CheckOrigin: func(r *http.Request) bool {
			return true // Allow all origins
		},
	}

	conn, err := upgrader.Upgrade(w, r, nil)
	if err != nil {
		logger.ErrorCF("websocket", "WebSocket upgrade failed", map[string]interface{}{
			"error":       err.Error(),
			"remote_addr": r.RemoteAddr,
		})
		return
	}

	// Store connection
	c.connMu.Lock()
	c.conn = conn
	c.clientID = fmt.Sprintf("client_%d", time.Now().Unix())
	c.connMu.Unlock()

	logger.InfoCF("websocket", "WebSocket client connected", map[string]interface{}{
		"client_id":   c.clientID,
		"remote_addr": r.RemoteAddr,
	})

	// Send welcome message
	welcomeMsg := ServerMessage{
		Type:      MessageTypeMessage,
		Role:      "system",
		Content:   fmt.Sprintf("Connected to NemesisBot WebSocket channel. Client ID: %s", c.clientID),
		Timestamp: time.Now(),
	}
	data, _ := json.Marshal(welcomeMsg)
	_ = conn.WriteMessage(websocket.TextMessage, data)

	// Handle connection in goroutine
	c.wg.Add(1)
	go c.handleConnection(conn)
}

// handleConnection handles the WebSocket connection
func (c *WebSocketChannel) handleConnection(conn *websocket.Conn) {
	defer c.wg.Done()

	defer func() {
		// Clean up connection
		c.connMu.Lock()
		c.conn = nil
		clientID := c.clientID
		c.clientID = ""
		c.connMu.Unlock()

		_ = conn.Close()
		logger.InfoCF("websocket", "WebSocket client disconnected", map[string]interface{}{
			"client_id": clientID,
		})
	}()

	// Set read deadline to very long value (effectively no timeout)
	if err := conn.SetReadDeadline(time.Now().Add(365 * 24 * time.Hour)); err != nil {
		logger.ErrorCF("websocket", "Failed to set read deadline", map[string]interface{}{
			"error": err.Error(),
		})
		return
	}

	// Set pong handler to update deadline
	conn.SetPongHandler(func(appData string) error {
		return conn.SetReadDeadline(time.Now().Add(365 * 24 * time.Hour))
	})

	// Message read loop
	logger.DebugC("websocket", "Starting message read loop")
	for {
		logger.DebugC("websocket", "Waiting for message from client")
		messageType, data, err := conn.ReadMessage()
		if err != nil {
			if websocket.IsUnexpectedCloseError(err, websocket.CloseGoingAway, websocket.CloseAbnormalClosure) {
				logger.ErrorCF("websocket", "WebSocket read error", map[string]interface{}{
					"error": err.Error(),
				})
			} else {
				logger.DebugCF("websocket", "WebSocket connection closed (normal close or timeout)", map[string]interface{}{
					"error": err.Error(),
				})
			}
			return
		}

		// Update read deadline to very long value (effectively no timeout)
		if err := conn.SetReadDeadline(time.Now().Add(365 * 24 * time.Hour)); err != nil {
			logger.ErrorCF("websocket", "Failed to update read deadline", map[string]interface{}{
				"error": err.Error(),
			})
			return
		}

		// Handle only text messages
		if messageType != websocket.TextMessage {
			logger.WarnCF("websocket", "Received non-text message", map[string]interface{}{
				"message_type": messageType,
			})
			continue
		}

		// Parse client message
		var clientMsg ClientMessage
		if err := json.Unmarshal(data, &clientMsg); err != nil {
			logger.ErrorCF("websocket", "Failed to parse client message", map[string]interface{}{
				"error": err.Error(),
				"data":  string(data),
			})
			c.sendError("Invalid message format")
			continue
		}

		// Handle different message types
		switch clientMsg.Type {
		case MessageTypeMessage:
			if clientMsg.Content == "" {
				logger.WarnC("websocket", "Received empty message")
				c.sendError("Message content cannot be empty")
				continue
			}

			// Send to message bus
			chatID := fmt.Sprintf("websocket:%s", c.clientID)
			logger.InfoCF("websocket", "Received message from client", map[string]interface{}{
				"content":  clientMsg.Content,
				"chat_id": chatID,
			})

			logger.DebugC("websocket", "Calling HandleMessage to send to message bus")
			c.HandleMessage(
				chatID,
				chatID,
				clientMsg.Content,
				[]string{},
				nil,
			)
			logger.DebugC("websocket", "HandleMessage returned, continuing message loop")

			// Sync to configured targets if enabled
			if c.config.SyncToWeb || len(c.config.SyncTo) > 0 {
				logger.DebugC("websocket", "Syncing message to configured targets")
				c.SyncToTargets("user", clientMsg.Content)
			}

		case MessageTypePing:
			// Respond with pong
			pongMsg := ServerMessage{
				Type:      MessageTypePong,
				Timestamp: time.Now(),
			}
			data, _ := json.Marshal(pongMsg)
			if err := conn.WriteMessage(websocket.TextMessage, data); err != nil {
				logger.ErrorCF("websocket", "Failed to send pong", map[string]interface{}{
					"error": err.Error(),
				})
			}

		default:
			logger.WarnCF("websocket", "Unknown message type", map[string]interface{}{
				"message_type": clientMsg.Type,
			})
			c.sendError(fmt.Sprintf("Unknown message type: %s", clientMsg.Type))
		}
		logger.DebugC("websocket", "Message processing complete, looping back to wait for next message")
	}
	logger.DebugC("websocket", "Message read loop ended")
}

// sendError sends an error message to the client
func (c *WebSocketChannel) sendError(message string) {
	c.connMu.RLock()
	conn := c.conn
	c.connMu.RUnlock()

	if conn == nil {
		return
	}

	errorMsg := ServerMessage{
		Type:      MessageTypeError,
		Error:     message,
		Timestamp: time.Now(),
	}
	data, err := json.Marshal(errorMsg)
	if err != nil {
		logger.ErrorCF("websocket", "Failed to marshal error message", map[string]interface{}{
			"error": err.Error(),
		})
		return
	}

	_ = conn.WriteMessage(websocket.TextMessage, data)
}
