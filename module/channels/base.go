// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package channels

import (
	"context"
	"fmt"
	"strings"
	"sync"
	"time"

	"github.com/276793422/NemesisBot/module/bus"
	"github.com/276793422/NemesisBot/module/logger"
)

type Channel interface {
	Name() string
	Start(ctx context.Context) error
	Stop(ctx context.Context) error
	Send(ctx context.Context, msg bus.OutboundMessage) error
	IsRunning() bool
	IsAllowed(senderID string) bool
	// SyncTarget management
	AddSyncTarget(name string, channel Channel) error
	RemoveSyncTarget(name string)
}

type BaseChannel struct {
	config      interface{}
	bus         *bus.MessageBus
	running     bool
	name        string
	allowList   []string
	syncTargets map[string]Channel
	syncMu      sync.RWMutex
}

func NewBaseChannel(name string, config interface{}, bus *bus.MessageBus, allowList []string) *BaseChannel {
	return &BaseChannel{
		config:    config,
		bus:       bus,
		name:      name,
		allowList: allowList,
		running:   false,
	}
}

func (c *BaseChannel) Name() string {
	return c.name
}

func (c *BaseChannel) IsRunning() bool {
	return c.running
}

func (c *BaseChannel) IsAllowed(senderID string) bool {
	if len(c.allowList) == 0 {
		return true
	}

	// Extract parts from compound senderID like "123456|username"
	idPart := senderID
	userPart := ""
	if idx := strings.Index(senderID, "|"); idx > 0 {
		idPart = senderID[:idx]
		userPart = senderID[idx+1:]
	}

	for _, allowed := range c.allowList {
		// Strip leading "@" from allowed value for username matching
		trimmed := strings.TrimPrefix(allowed, "@")
		allowedID := trimmed
		allowedUser := ""
		if idx := strings.Index(trimmed, "|"); idx > 0 {
			allowedID = trimmed[:idx]
			allowedUser = trimmed[idx+1:]
		}

		// Support either side using "id|username" compound form.
		// This keeps backward compatibility with legacy Telegram allowlist entries.
		if senderID == allowed ||
			idPart == allowed ||
			senderID == trimmed ||
			idPart == trimmed ||
			idPart == allowedID ||
			(allowedUser != "" && senderID == allowedUser) ||
			(userPart != "" && (userPart == allowed || userPart == trimmed || userPart == allowedUser)) {
			return true
		}
	}

	return false
}

func (c *BaseChannel) HandleMessage(senderID, chatID, content string, media []string, metadata map[string]string) {
	if !c.IsAllowed(senderID) {
		return
	}

	msg := bus.InboundMessage{
		Channel:  c.name,
		SenderID: senderID,
		ChatID:   chatID,
		Content:  content,
		Media:    media,
		Metadata: metadata,
	}

	c.bus.PublishInbound(msg)
}

func (c *BaseChannel) setRunning(running bool) {
	c.running = running
}

// AddSyncTarget adds a channel as a sync target
func (c *BaseChannel) AddSyncTarget(name string, channel Channel) error {
	// Prevent self-sync
	if name == c.name {
		return fmt.Errorf("channel cannot sync to itself")
	}

	c.syncMu.Lock()
	defer c.syncMu.Unlock()

	if c.syncTargets == nil {
		c.syncTargets = make(map[string]Channel)
	}

	c.syncTargets[name] = channel
	logger.DebugCF(c.name, "Sync target added", map[string]interface{}{
		"target": name,
	})
	return nil
}

// RemoveSyncTarget removes a sync target by name
func (c *BaseChannel) RemoveSyncTarget(name string) {
	c.syncMu.Lock()
	defer c.syncMu.Unlock()

	delete(c.syncTargets, name)
	logger.DebugCF(c.name, "Sync target removed", map[string]interface{}{
		"target": name,
	})
}

// SyncToTargets sends a message to all configured sync targets
func (c *BaseChannel) SyncToTargets(role, content string) {
	c.syncMu.RLock()
	defer c.syncMu.RUnlock()

	logger.DebugCF(c.name, "SyncToTargets called", map[string]interface{}{
		"role":        role,
		"content_len": len(content),
		"num_targets": len(c.syncTargets),
	})

	if len(c.syncTargets) == 0 {
		logger.DebugCF(c.name, "No sync targets configured, skipping", nil)
		return
	}

	for targetName, targetCh := range c.syncTargets {
		// Skip self-sync (double-check)
		if targetName == c.name {
			continue
		}

		// Create message for target
		msg := bus.OutboundMessage{
			Channel: targetName,
			Content: content,
		}

		// For web channel, use broadcast to reach all web clients
		if targetName == "web" {
			msg.ChatID = "web:broadcast"
		}

		logger.DebugCF(c.name, "Sending to sync target", map[string]interface{}{
			"target":      targetName,
			"content_len": len(content),
			"chat_id":     msg.ChatID,
		})

		// Send with timeout to avoid blocking
		ctx, cancel := context.WithTimeout(context.Background(), 3*time.Second)
		if err := targetCh.Send(ctx, msg); err != nil {
			logger.WarnCF(c.name, "Failed to sync to target", map[string]interface{}{
				"target": targetName,
				"error":  err.Error(),
			})
		}
		cancel()
	}
}
