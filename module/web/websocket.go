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

	"github.com/276793422/NemesisBot/module/logger"
	"github.com/gorilla/websocket"
)

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
	conn   *websocket.Conn
	queue  chan sendRequest
	ctx    context.Context
	cancel context.CancelFunc
	wg     sync.WaitGroup
	once   sync.Once
}

// newSendQueue creates a new send queue for the connection
func newSendQueue(conn *websocket.Conn) *sendQueue {
	ctx, cancel := context.WithCancel(context.Background())

	sq := &sendQueue{
		conn:   conn,
		queue:  make(chan sendRequest, 256), // Buffer for pending sends
		ctx:    ctx,
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
				"session_id":   session.ID,
			})
			continue
		}

		// Parse protocol message
		protoMsg, err := ParseProtocolMessage(data)
		if err != nil {
			logger.ErrorCF("web", "Failed to parse protocol message", map[string]interface{}{
				"error":      err.Error(),
				"data":       string(data),
				"session_id": session.ID,
			})
			sendErrorViaQueue(sq, "Invalid protocol message format")
			continue
		}

		// Dispatch by type
		switch protoMsg.Type {
		case "message":
			handleMessageModule(session, sq, messageChan, protoMsg)
		case "system":
			handleSystemModule(sq, protoMsg)
		default:
			logger.WarnCF("web", "Unknown protocol type", map[string]interface{}{
				"type":       protoMsg.Type,
				"session_id": session.ID,
			})
			sendErrorViaQueue(sq, fmt.Sprintf("Unknown protocol type: %s", protoMsg.Type))
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
	Metadata  map[string]string // Additional metadata (e.g. request_type)
}

// sendProtocolMessageViaQueue sends a ProtocolMessage using the send queue.
func sendProtocolMessageViaQueue(sq *sendQueue, msg *ProtocolMessage) error {
	data, err := msg.ToJSON()
	if err != nil {
		return fmt.Errorf("failed to marshal protocol message: %w", err)
	}
	return sq.send(websocket.TextMessage, data)
}

// sendErrorViaQueue sends an error message using the send queue (new protocol).
func sendErrorViaQueue(sq *sendQueue, message string) {
	errorMsg, _ := NewProtocolMessage("system", "error", "notify", map[string]string{
		"content": message,
	})
	_ = sendProtocolMessageViaQueue(sq, errorMsg)
}

// BroadcastToSession sends a message to a specific session using the new protocol.
func BroadcastToSession(sessionMgr *SessionManager, sessionID string, role, content string) error {
	logger.DebugCF("web", "BroadcastToSession called",
		map[string]interface{}{
			"session_id":      sessionID,
			"role":            role,
			"content_len":     len(content),
			"content_preview": content[:min(100, len(content))],
		})

	msg, err := NewProtocolMessage("message", "chat", "receive", map[string]string{
		"role":    role,
		"content": content,
	})
	if err != nil {
		logger.ErrorCF("web", "Failed to create protocol message",
			map[string]interface{}{
				"session_id": sessionID,
				"error":      err.Error(),
			})
		return fmt.Errorf("failed to create protocol message: %w", err)
	}

	data, err := msg.ToJSON()
	if err != nil {
		logger.ErrorCF("web", "Failed to marshal message",
			map[string]interface{}{
				"session_id": sessionID,
				"error":      err.Error(),
			})
		return fmt.Errorf("failed to marshal message: %w", err)
	}

	logger.DebugCF("web", "Message marshaled, broadcasting to session",
		map[string]interface{}{
			"session_id": sessionID,
			"data_len":   len(data),
		})

	err = sessionMgr.Broadcast(sessionID, data)
	if err != nil {
		logger.ErrorCF("web", "Failed to broadcast to session",
			map[string]interface{}{
				"session_id": sessionID,
				"error":      err.Error(),
			})
		return err
	}

	logger.InfoCF("web", "BroadcastToSession completed successfully",
		map[string]interface{}{
			"session_id": sessionID,
			"role":       role,
		})

	return nil
}

// handleMessageModule dispatches messages with type=="message" in the new protocol.
func handleMessageModule(session *Session, sq *sendQueue, messageChan chan<- IncomingMessage, msg *ProtocolMessage) {
	switch msg.Module {
	case "chat":
		switch msg.Cmd {
		case "send":
			handleChatSend(session, sq, messageChan, msg)
		case "history_request":
			handleHistoryRequest(session, sq, messageChan, msg)
		default:
			logger.WarnCF("web", "Unknown chat cmd", map[string]interface{}{
				"cmd":        msg.Cmd,
				"session_id": session.ID,
			})
			sendErrorViaQueue(sq, fmt.Sprintf("Unknown chat cmd: %s", msg.Cmd))
		}
	default:
		logger.WarnCF("web", "Unknown message module", map[string]interface{}{
			"module":     msg.Module,
			"session_id": session.ID,
		})
		sendErrorViaQueue(sq, fmt.Sprintf("Unknown message module: %s", msg.Module))
	}
}

// handleChatSend processes a chat.send message (new protocol).
func handleChatSend(session *Session, sq *sendQueue, messageChan chan<- IncomingMessage, msg *ProtocolMessage) {
	var data struct {
		Content string `json:"content"`
	}
	if err := msg.DecodeData(&data); err != nil {
		sendErrorViaQueue(sq, "Invalid chat.send data")
		return
	}

	if data.Content == "" {
		sendErrorViaQueue(sq, "Message content cannot be empty")
		return
	}

	select {
	case messageChan <- IncomingMessage{
		SessionID: session.ID,
		SenderID:  session.SenderID,
		ChatID:    session.ChatID,
		Content:   data.Content,
		Timestamp: time.Now(),
	}:
		logger.DebugCF("web", "Message forwarded to channel (new protocol)", map[string]interface{}{
			"session_id": session.ID,
			"content":    data.Content,
		})
	default:
		logger.WarnC("web", "Message channel full, dropping message")
		sendErrorViaQueue(sq, "Server busy, please try again")
	}
}

// handleHistoryRequest processes a chat.history_request message by routing it through the bus.
func handleHistoryRequest(session *Session, sq *sendQueue, messageChan chan<- IncomingMessage, msg *ProtocolMessage) {
	var reqData HistoryRequestData
	if err := msg.DecodeData(&reqData); err != nil {
		sendErrorViaQueue(sq, "Invalid history_request data")
		return
	}

	// Serialize request data as JSON content for the agent to parse
	payload, err := json.Marshal(reqData)
	if err != nil {
		sendErrorViaQueue(sq, "Failed to serialize history request")
		return
	}

	select {
	case messageChan <- IncomingMessage{
		SessionID: session.ID,
		SenderID:  session.SenderID,
		ChatID:    session.ChatID,
		Content:   string(payload),
		Timestamp: time.Now(),
		Metadata:  map[string]string{"request_type": "history"},
	}:
		logger.DebugCF("web", "History request forwarded to channel", map[string]interface{}{
			"session_id": session.ID,
			"request_id": reqData.RequestID,
		})
	default:
		logger.WarnC("web", "Message channel full, dropping history request")
		sendErrorViaQueue(sq, "Server busy, please try again")
	}
}

// handleSystemModule dispatches messages with type=="system" in the new protocol.
func handleSystemModule(sq *sendQueue, msg *ProtocolMessage) {
	switch msg.Module {
	case "heartbeat":
		switch msg.Cmd {
		case "ping":
			pong, _ := NewProtocolMessage("system", "heartbeat", "pong", map[string]interface{}{})
			data, _ := pong.ToJSON()
			_ = sq.send(websocket.TextMessage, data)
		default:
			sendErrorViaQueue(sq, fmt.Sprintf("Unknown heartbeat cmd: %s", msg.Cmd))
		}
	case "error":
		switch msg.Cmd {
		case "notify":
			// Client sent an error notification, log it
			logger.WarnCF("web", "Client error notification", map[string]interface{}{
				"data": string(msg.Data),
			})
		default:
			sendErrorViaQueue(sq, fmt.Sprintf("Unknown error cmd: %s", msg.Cmd))
		}
	default:
		sendErrorViaQueue(sq, fmt.Sprintf("Unknown system module: %s", msg.Module))
	}
}

func min(a, b int) int {
	if a < b {
		return a
	}
	return b
}
