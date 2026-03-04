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

// RPCChannel allows RPC handlers to use the bot's LLM processing
// It implements the Channel interface but provides an additional Input() method
// for RPC handlers to submit requests and wait for responses
type RPCChannel struct {
	base *BaseChannel

	// Request tracking
	mu          sync.RWMutex
	pendingReqs map[string]*pendingRequest // correlation_id → request

	// Configuration
	requestTimeout  time.Duration // LLM processing timeout
	cleanupInterval time.Duration // Cleanup interval for expired requests

	// Lifecycle
	running bool
	stopCh  chan struct{}
	wg      sync.WaitGroup
}

// pendingRequest represents a pending LLM request from RPC
type pendingRequest struct {
	correlationID string
	responseCh    chan string
	createdAt     time.Time
	timeout       time.Duration
}

// RPCChannelConfig holds configuration for RPCChannel
type RPCChannelConfig struct {
	MessageBus      *bus.MessageBus
	RequestTimeout  time.Duration // LLM processing timeout (default: 60s)
	CleanupInterval time.Duration // Cleanup interval (default: 30s)
}

// NewRPCChannel creates a new RPC channel
func NewRPCChannel(cfg *RPCChannelConfig) (*RPCChannel, error) {
	if cfg == nil {
		return nil, fmt.Errorf("config cannot be nil")
	}
	if cfg.MessageBus == nil {
		return nil, fmt.Errorf("message bus cannot be nil")
	}

	// Set defaults
	if cfg.RequestTimeout == 0 {
		cfg.RequestTimeout = 60 * time.Second
	}
	if cfg.CleanupInterval == 0 {
		cfg.CleanupInterval = 30 * time.Second
	}

	base := NewBaseChannel("rpc", nil, cfg.MessageBus, nil)

	return &RPCChannel{
		base:            base,
		pendingReqs:     make(map[string]*pendingRequest),
		requestTimeout:  cfg.RequestTimeout,
		cleanupInterval: cfg.CleanupInterval,
		stopCh:          make(chan struct{}),
	}, nil
}

// Name returns the channel name
func (ch *RPCChannel) Name() string {
	return ch.base.Name()
}

// Start starts the RPC channel
func (ch *RPCChannel) Start(ctx context.Context) error {
	if ch.running {
		return fmt.Errorf("RPC channel already running")
	}

	ch.running = true
	ch.base.setRunning(true)

	logger.InfoC("rpc", "Starting RPC channel")

	// Start outbound listener
	ch.wg.Add(1)
	go ch.outboundListener(ctx)

	// Start cleanup goroutine
	ch.wg.Add(1)
	go ch.cleanupLoop()

	logger.InfoC("rpc", "RPC channel started")
	return nil
}

// Stop stops the RPC channel
func (ch *RPCChannel) Stop(ctx context.Context) error {
	if !ch.running {
		return nil
	}

	logger.InfoC("rpc", "Stopping RPC channel")
	ch.running = false
	ch.base.setRunning(false)

	// Signal goroutines to stop
	close(ch.stopCh)

	// Wait for goroutines to finish (with timeout)
	done := make(chan struct{})
	go func() {
		ch.wg.Wait()
		close(done)
	}()

	select {
	case <-done:
		// All goroutines stopped
	case <-time.After(5 * time.Second):
		logger.WarnC("rpc", "Timeout waiting for goroutines to stop")
	case <-ctx.Done():
		logger.WarnC("rpc", "Context canceled while stopping")
	}

	// Clear all pending requests
	ch.mu.Lock()
	for correlationID, req := range ch.pendingReqs {
		close(req.responseCh)
		logger.DebugCF("rpc", "Cleared pending request", map[string]interface{}{
			"correlation_id": correlationID,
		})
	}
	ch.pendingReqs = make(map[string]*pendingRequest)
	ch.mu.Unlock()

	logger.InfoC("rpc", "RPC channel stopped")
	return nil
}

// Send implements Channel interface - not used for RPC channel but required by interface
func (ch *RPCChannel) Send(ctx context.Context, msg bus.OutboundMessage) error {
	// RPC channel doesn't actively send messages
	// Responses are delivered through the pending request mechanism
	return nil
}

// IsRunning returns true if the channel is running
func (ch *RPCChannel) IsRunning() bool {
	return ch.running
}

// IsAllowed implements Channel interface - RPC channel allows all internal requests
func (ch *RPCChannel) IsAllowed(senderID string) bool {
	return true
}

// AddSyncTarget implements Channel interface - not used for RPC
func (ch *RPCChannel) AddSyncTarget(name string, channel Channel) error {
	return ch.base.AddSyncTarget(name, channel)
}

// RemoveSyncTarget implements Channel interface - not used for RPC
func (ch *RPCChannel) RemoveSyncTarget(name string) {
	ch.base.RemoveSyncTarget(name)
}

// Input sends an inbound message to the MessageBus and returns a response channel
// This is the main interface for RPC handlers
// The correlation ID is used to match the response
func (ch *RPCChannel) Input(ctx context.Context, inbound *bus.InboundMessage) (<-chan string, error) {
	if !ch.running {
		return nil, fmt.Errorf("RPC channel is not running")
	}

	// Generate correlation ID if not set
	if inbound.CorrelationID == "" {
		inbound.CorrelationID = generateCorrelationID()
	}
	inbound.Channel = ch.Name() // Set channel to "rpc"

	// Create pending request
	respCh := make(chan string, 1)

	ch.mu.Lock()
	ch.pendingReqs[inbound.CorrelationID] = &pendingRequest{
		correlationID: inbound.CorrelationID,
		responseCh:    respCh,
		createdAt:     time.Now(),
		timeout:       ch.getRequestTimeout(inbound.Metadata),
	}
	ch.mu.Unlock()

	logger.DebugCF("rpc", "Registered pending request", map[string]interface{}{
		"correlation_id": inbound.CorrelationID,
		"chat_id":        inbound.ChatID,
		"content_len":    len(inbound.Content),
	})

	// Send to MessageBus
	ch.base.bus.PublishInbound(*inbound)

	return respCh, nil
}

// outboundListener listens for outbound messages and delivers them to waiting RPC handlers
func (ch *RPCChannel) outboundListener(ctx context.Context) {
	defer ch.wg.Done()

	logger.DebugC("rpc", "Outbound listener started")

	for {
		select {
		case <-ch.stopCh:
			logger.DebugC("rpc", "Outbound listener stopped (signal)")
			return

		case <-ctx.Done():
			logger.DebugC("rpc", "Outbound listener stopped (context)")
			return

		case msg, ok := <-ch.base.bus.OutboundChannel():
			if !ok {
				logger.DebugC("rpc", "Outbound channel closed")
				return
			}

			// Only process messages from this channel
			if msg.Channel != ch.Name() {
				continue
			}

			// Extract correlation ID from content
			// Format: "[rpc:correlation_id] actual response"
			correlationID := extractCorrelationID(msg.Content)
			if correlationID == "" {
				logger.DebugCF("rpc", "No correlation ID in message", map[string]interface{}{
					"content": msg.Content,
				})
				continue
			}

			// Find pending request and deliver response
			ch.mu.RLock()
			req, exists := ch.pendingReqs[correlationID]
			ch.mu.RUnlock()

			if exists {
				actualContent := removeCorrelationID(msg.Content)
				select {
				case req.responseCh <- actualContent:
					logger.DebugCF("rpc", "Delivered response", map[string]interface{}{
						"correlation_id": correlationID,
						"content_len":    len(actualContent),
					})
				case <-time.After(time.Second):
					logger.WarnCF("rpc", "Failed to deliver response (channel full or closed)", map[string]interface{}{
						"correlation_id": correlationID,
					})
				}
			} else {
				logger.DebugCF("rpc", "No pending request for correlation ID", map[string]interface{}{
					"correlation_id": correlationID,
				})
			}
		}
	}
}

// cleanupLoop periodically removes expired pending requests
func (ch *RPCChannel) cleanupLoop() {
	defer ch.wg.Done()

	logger.DebugC("rpc", "Cleanup loop started")
	ticker := time.NewTicker(ch.cleanupInterval)
	defer ticker.Stop()

	for {
		select {
		case <-ch.stopCh:
			logger.DebugC("rpc", "Cleanup loop stopped")
			return
		case <-ticker.C:
			ch.cleanupExpiredRequests()
		}
	}
}

// cleanupExpiredRequests removes pending requests that have timed out
func (ch *RPCChannel) cleanupExpiredRequests() {
	ch.mu.Lock()
	defer ch.mu.Unlock()

	now := time.Now()
	expiredCount := 0

	for correlationID, req := range ch.pendingReqs {
		if now.Sub(req.createdAt) > req.timeout {
			// Request expired
			close(req.responseCh)
			delete(ch.pendingReqs, correlationID)
			expiredCount++
			logger.DebugCF("rpc", "Expired pending request", map[string]interface{}{
				"correlation_id": correlationID,
				"age_seconds":    now.Sub(req.createdAt).Seconds(),
			})
		}
	}

	if expiredCount > 0 {
		logger.DebugCF("rpc", "Cleaned expired requests", map[string]interface{}{
			"count": expiredCount,
		})
	}
}

// getRequestTimeout returns the timeout for a request (can be customized via metadata)
func (ch *RPCChannel) getRequestTimeout(metadata map[string]string) time.Duration {
	if metadata != nil {
		if timeoutStr, ok := metadata["rpc_timeout"]; ok {
			if duration, err := time.ParseDuration(timeoutStr); err == nil {
				return duration
			}
		}
	}
	return ch.requestTimeout
}

// generateCorrelationID generates a unique correlation ID
func generateCorrelationID() string {
	return fmt.Sprintf("rpc-%d", time.Now().UnixNano())
}

// extractCorrelationID extracts correlation ID from content
// Format: "[rpc:correlation_id] actual content"
func extractCorrelationID(content string) string {
	if !strings.HasPrefix(content, "[rpc:") {
		return ""
	}

	end := strings.Index(content, "]")
	if end == -1 {
		return ""
	}

	if end <= 5 { // "[rpc:" is 5 chars
		return ""
	}

	return content[5:end] // Extract ID from "[rpc:id]"
}

// removeCorrelationID removes correlation ID prefix from content
func removeCorrelationID(content string) string {
	if !strings.HasPrefix(content, "[rpc:") {
		return content
	}

	end := strings.Index(content, "]")
	if end == -1 {
		return content
	}

	// Skip "[rpc:id] " and return actual content
	if end+1 < len(content) && content[end+1] == ' ' {
		return content[end+2:]
	}
	return content[end+1:]
}
