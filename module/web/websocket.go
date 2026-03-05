// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
// Web Channel Module - WebSocket Handler with Thread-Safe Send Queue

package web

import (
	"context"
	"encoding/json"
	"fmt"
	"net/http"
	"sync"
	"time"

	"github.com/gorilla/websocket"
	"github.com/276793422/NemesisBot/module/logger"
)

// WebSocket message types
const (
	MessageTypeMessage = "message"
	MessageTypePing    = "ping"
	MessageTypePong    = "pong"
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
}

// WebSocketUpgrader handles the WebSocket upgrade
var WebSocketUpgrader = websocket.Upgrader{
	ReadBufferSize:  1024,
	WriteBufferSize: 1024,
	CheckOrigin: func(r *http.Request) bool {
		// Allow all origins for now (can be restricted later)
		return true
	},
}

// sendRequest represents a message to be sent
type sendRequest struct {
	messageType int
	data        []byte
	result      chan error
}

// sendQueue handles concurrent writes to WebSocket safely
type sendQueue struct {
	conn    *websocket.Conn
	queue    chan sendRequest
	ctx      context.Context
	cancel   context.CancelFunc
	wg       sync.WaitGroup
	once     sync.Once
}

// newSendQueue creates a new send queue for the connection
func newSendQueue(conn *websocket.Conn) *sendQueue {
	ctx, cancel := context.WithCancel(context.Background())

	sq := &sendQueue{
		conn: conn,
		queue: make(chan sendRequest, 256), // Buffer for pending sends
		ctx:  ctx,
		cancel: cancel,
	}

	sq.wg.Add(1)
	go sq.process()

	return sq
}

// process runs the send loop (single goroutine for all writes)
func (sq *sendQueue) process() {
	defer sq.wg.Done()

	for {
		select {
		case <-sq.ctx.Done():
			return
		case req := <-sq.queue:
			// Set write deadline for each send
			err := sq.conn.SetWriteDeadline(time.Now().Add(10 * time.Second))
			if err != nil {
				if req.result != nil {
					req.result <- err
				}
				continue
			}

			// Write message
			err = sq.conn.WriteMessage(req.messageType, req.data)
			if req.result != nil {
				req.result <- err
			}
		}
	}
}

// send adds a message to the send queue
func (sq *sendQueue) send(messageType int, data []byte) error {
	if sq == nil {
		return fmt.Errorf("send queue not initialized")
	}

	req := sendRequest{
		messageType: messageType,
		data:        data,
		result:      make(chan error, 1),
	}

	select {
	case sq.queue <- req:
		return <-req.result
	case <-sq.ctx.Done():
		return fmt.Errorf("send queue stopped")
	case <-time.After(5 * time.Second):
		return fmt.Errorf("send queue timeout")
	}
}

// stop stops the send queue
func (sq *sendQueue) stop() {
	sq.once.Do(func() {
		sq.cancel()
		sq.wg.Wait()
	})
}

// HandleWebSocket handles a WebSocket connection with thread-safe sending
func HandleWebSocket(session *Session, sessionMgr *SessionManager, messageChan chan<- IncomingMessage, authToken string) error {
	defer func() {
		if r := recover(); r != nil {
			logger.ErrorCF("web", "Panic in WebSocket handler", map[string]interface{}{
				"error":      fmt.Sprintf("%v", r),
				"session_id": session.ID,
			})
		}
	}()

	// Create send queue for this connection
	sq := newSendQueue(session.Conn)
	defer sq.stop()

	// Store send queue in session for outbound messages
	session.mu.Lock()
	session.sendQueue = sq
	session.mu.Unlock()

	// Configure pong handler to update read deadline on each pong
	// Note: We don't send pong here, gorilla/websocket does it automatically
	session.Conn.SetPongHandler(func(appData string) error {
		session.mu.Lock()
		defer session.mu.Unlock()

		// Update last active time
		session.LastActive = time.Now()

		// Extend read deadline - connection stays alive as long as we receive pings/pongs
		// Set to 90 seconds to give some buffer
		err := session.Conn.SetReadDeadline(time.Now().Add(90 * time.Second))
		if err != nil {
			return fmt.Errorf("failed to set read deadline: %w", err)
		}

		return nil
	})

	// Set initial read deadline
	err := session.Conn.SetReadDeadline(time.Now().Add(90 * time.Second))
	if err != nil {
		return fmt.Errorf("failed to set read deadline: %w", err)
	}

	// Start message read loop
	for {
		messageType, data, err := session.Conn.ReadMessage()
		if err != nil {
			if websocket.IsUnexpectedCloseError(err, websocket.CloseGoingAway, websocket.CloseAbnormalClosure) {
				logger.ErrorCF("web", "WebSocket read error", map[string]interface{}{
					"error":      err.Error(),
					"session_id": session.ID,
				})
			}
			return nil // Normal closure or expected error
		}

		// Update last active time and extend read deadline on each message
		session.mu.Lock()
		session.LastActive = time.Now()
		session.mu.Unlock()

		// Extend read deadline after each message
		err = session.Conn.SetReadDeadline(time.Now().Add(90 * time.Second))
		if err != nil {
			logger.ErrorCF("web", "Failed to update read deadline", map[string]interface{}{
				"error":      err.Error(),
				"session_id": session.ID,
			})
			return err
		}

		// Handle binary messages (not supported yet)
		if messageType != websocket.TextMessage {
			logger.WarnCF("web", "Received non-text message", map[string]interface{}{
				"message_type": messageType,
				"session_id":  session.ID,
			})
			continue
		}

		// Parse client message
		var clientMsg ClientMessage
		if err := json.Unmarshal(data, &clientMsg); err != nil {
			logger.ErrorCF("web", "Failed to parse client message", map[string]interface{}{
				"error":      err.Error(),
				"data":       string(data),
				"session_id": session.ID,
			})
			// Send error message to client
			sendErrorViaQueue(sq, "Invalid message format")
			continue
		}

		// Handle different message types
		switch clientMsg.Type {
		case MessageTypeMessage:
			// Validate content
			if clientMsg.Content == "" {
				logger.WarnCF("web", "Received empty message", map[string]interface{}{
					"session_id": session.ID,
				})
				sendErrorViaQueue(sq, "Message content cannot be empty")
				continue
			}

			// Send to message channel for processing
			select {
			case messageChan <- IncomingMessage{
				SessionID: session.ID,
				SenderID:  session.SenderID,
				ChatID:    session.ChatID,
				Content:   clientMsg.Content,
				Timestamp: clientMsg.Timestamp,
			}:
				logger.DebugCF("web", "Message forwarded to channel", map[string]interface{}{
					"session_id": session.ID,
					"content":    clientMsg.Content,
				})
			default:
				logger.WarnC("web", "Message channel full, dropping message")
				sendErrorViaQueue(sq, "Server busy, please try again")
			}

		case MessageTypePing:
			// Respond with pong using send queue
			pongMsg := ServerMessage{
				Type:      MessageTypePong,
				Timestamp: time.Now(),
			}
			if err := sendServerMessageViaQueue(sq, pongMsg); err != nil {
				logger.ErrorCF("web", "Failed to send pong", map[string]interface{}{
					"error":      err.Error(),
					"session_id": session.ID,
				})
				return err
			}

		default:
			logger.WarnCF("web", "Unknown message type", map[string]interface{}{
				"message_type": clientMsg.Type,
				"session_id":  session.ID,
			})
			sendErrorViaQueue(sq, fmt.Sprintf("Unknown message type: %s", clientMsg.Type))
		}
	}
}

// IncomingMessage represents a message received from a WebSocket client
type IncomingMessage struct {
	SessionID string
	SenderID  string
	ChatID    string
	Content   string
	Timestamp time.Time
}

// sendServerMessageViaQueue sends a message using the send queue
func sendServerMessageViaQueue(sq *sendQueue, msg ServerMessage) error {
	data, err := json.Marshal(msg)
	if err != nil {
		return fmt.Errorf("failed to marshal message: %w", err)
	}

	return sq.send(websocket.TextMessage, data)
}

// sendErrorViaQueue sends an error message using the send queue
func sendErrorViaQueue(sq *sendQueue, message string) {
	errorMsg := ServerMessage{
		Type:      "error",
		Content:   message,
		Timestamp: time.Now(),
	}
	_ = sendServerMessageViaQueue(sq, errorMsg)
}

// BroadcastToSession sends a message to a specific session
func BroadcastToSession(sessionMgr *SessionManager, sessionID string, role, content string) error {
	logger.DebugCF("web", "BroadcastToSession called",
		map[string]interface{}{
			"session_id": sessionID,
			"role": role,
			"content_len": len(content),
			"content_preview": content[:min(100, len(content))],
		})

	msg := ServerMessage{
		Type:      MessageTypeMessage,
		Role:      role,
		Content:   content,
		Timestamp: time.Now(),
	}

	data, err := json.Marshal(msg)
	if err != nil {
		logger.ErrorCF("web", "Failed to marshal message",
			map[string]interface{}{
				"session_id": sessionID,
				"error": err.Error(),
			})
		return fmt.Errorf("failed to marshal message: %w", err)
	}

	logger.DebugCF("web", "Message marshaled, broadcasting to session",
		map[string]interface{}{
			"session_id": sessionID,
			"data_len": len(data),
		})

	err = sessionMgr.Broadcast(sessionID, data)
	if err != nil {
		logger.ErrorCF("web", "Failed to broadcast to session",
			map[string]interface{}{
				"session_id": sessionID,
				"error": err.Error(),
			})
		return err
	}

	logger.InfoCF("web", "BroadcastToSession completed successfully",
		map[string]interface{}{
			"session_id": sessionID,
			"role": role,
		})

	return nil
}

func min(a, b int) int {
	if a < b {
		return a
	}
	return b
}
