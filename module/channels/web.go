// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
// Web Channel - WebSocket-based chat channel

package channels

import (
	"context"
	"fmt"
	"time"

	"github.com/276793422/NemesisBot/module/bus"
	"github.com/276793422/NemesisBot/module/config"
	"github.com/276793422/NemesisBot/module/logger"
	"github.com/276793422/NemesisBot/module/web"
)

// WebChannel implements a WebSocket-based chat channel
type WebChannel struct {
	*BaseChannel
	config     *config.WebChannelConfig
	server     *web.Server
	sessionMgr *web.SessionManager
	running    bool
}

// NewWebChannel creates a new web channel
func NewWebChannel(cfg *config.WebChannelConfig, messageBus *bus.MessageBus) (*WebChannel, error) {
	// Create session manager with configured timeout
	sessionTimeout := time.Duration(cfg.SessionTimeout) * time.Second
	if sessionTimeout == 0 {
		sessionTimeout = 1 * time.Hour // Default 1 hour
	}
	sessionMgr := web.NewSessionManager(sessionTimeout)

	base := NewBaseChannel("web", cfg, messageBus, cfg.AllowFrom)

	return &WebChannel{
		BaseChannel: base,
		config:      cfg,
		sessionMgr:  sessionMgr,
		running:     false,
	}, nil
}

// Start starts the web channel HTTP server
func (c *WebChannel) Start(ctx context.Context) error {
	logger.InfoCF("web", "Starting web channel", map[string]interface{}{
		"host": c.config.Host,
		"port": c.config.Port,
		"path": c.config.Path,
	})

	// Create and start server
	c.server = web.NewServer(web.ServerConfig{
		Host:       c.config.Host,
		Port:       c.config.Port,
		WSPath:     c.config.Path,
		AuthToken:  c.config.AuthToken,
		SessionMgr: c.sessionMgr,
		Bus:        c.bus,
	})

	// Start server in goroutine
	errChan := make(chan error, 1)
	go func() {
		if err := c.server.Start(ctx); err != nil {
			errChan <- err
		}
	}()

	// Wait briefly to ensure server starts successfully
	select {
	case err := <-errChan:
		return err
	case <-time.After(100 * time.Millisecond):
		// Server started successfully
		c.setRunning(true)
		logger.InfoCF("web", "Web channel started", map[string]interface{}{
			"url": fmt.Sprintf("http://%s:%d", c.config.Host, c.config.Port),
		})
		return nil
	}
}

// Stop stops the web channel
func (c *WebChannel) Stop(ctx context.Context) error {
	logger.InfoC("web", "Stopping web channel")

	c.setRunning(false)

	// Shutdown server
	if c.server != nil {
		if err := c.server.Shutdown(ctx); err != nil {
			logger.ErrorCF("web", "Error stopping server", map[string]interface{}{
				"error": err.Error(),
			})
			return err
		}
	}

	// Shutdown session manager
	c.sessionMgr.Shutdown()

	logger.InfoC("web", "Web channel stopped")
	return nil
}

// Send sends a message to a WebSocket client
func (c *WebChannel) Send(ctx context.Context, msg bus.OutboundMessage) error {
	if !c.IsRunning() {
		logger.WarnCF("web", "Web channel not running, cannot send message",
			map[string]interface{}{
				"chat_id":     msg.ChatID,
				"content_len": len(msg.Content),
			})
		return fmt.Errorf("web channel not running")
	}

	// Handle broadcast to all sessions
	if msg.ChatID == "web:broadcast" {
		logger.DebugCF("web", "Broadcasting to all sessions",
			map[string]interface{}{
				"content_len": len(msg.Content),
			})
		return c.BroadcastToAll(msg.Content)
	}

	// Extract session ID from chat ID (format: web:<session-id>)
	var sessionID string
	if len(msg.ChatID) > 4 && msg.ChatID[:4] == "web:" {
		sessionID = msg.ChatID[4:]
	} else {
		logger.ErrorCF("web", "Invalid chat ID format",
			map[string]interface{}{
				"chat_id":         msg.ChatID,
				"expected_format": "web:<session-id>",
			})
		return fmt.Errorf("invalid chat ID format: %s", msg.ChatID)
	}

	logger.DebugCF("web", "Sending message to session",
		map[string]interface{}{
			"session_id":  sessionID,
			"chat_id":     msg.ChatID,
			"content_len": len(msg.Content),
		})

	// Send message to session
	if err := c.server.SendToSession(sessionID, "assistant", msg.Content); err != nil {
		logger.ErrorCF("web", "Failed to send message to session",
			map[string]interface{}{
				"error":      err.Error(),
				"session_id": sessionID,
				"chat_id":    msg.ChatID,
			})
		return err
	}

	logger.InfoCF("web", "Message sent to session successfully",
		map[string]interface{}{
			"session_id": sessionID,
			"chat_id":    msg.ChatID,
		})

	return nil
}

// BroadcastToAll sends a message to all active web sessions
func (c *WebChannel) BroadcastToAll(content string) error {
	sessions := c.sessionMgr.GetAllSessions()
	logger.DebugCF("web", "Broadcasting to all sessions", map[string]interface{}{
		"session_count": len(sessions),
		"content":       content,
	})

	for _, session := range sessions {
		if err := c.server.SendToSession(session.ID, "assistant", content); err != nil {
			logger.WarnCF("web", "Failed to broadcast to session", map[string]interface{}{
				"error":      err.Error(),
				"session_id": session.ID,
			})
		}
	}
	return nil
}

// IsRunning returns whether the channel is running
func (c *WebChannel) IsRunning() bool {
	return c.running
}

// setRunning sets the running state
func (c *WebChannel) setRunning(running bool) {
	c.running = running
}
