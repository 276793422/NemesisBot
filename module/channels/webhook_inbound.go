// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
// Webhook Inbound Channel - receives HTTP POST requests and converts them to InboundMessages

package channels

import (
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"strings"
	"sync"
	"time"

	"github.com/276793422/NemesisBot/module/bus"
	"github.com/276793422/NemesisBot/module/logger"
	"github.com/276793422/NemesisBot/module/utils"
)

// WebhookInboundConfig configures the webhook inbound channel.
type WebhookInboundConfig struct {
	ListenAddr  string   // ":9090"
	Path        string   // "/webhook/incoming"
	APIKey      string   // optional, empty = no auth
	ChannelName string   // "webhook"
	AllowFrom   []string // sender allowlist
}

// webhookRequest is the JSON body accepted by the webhook endpoint.
type webhookRequest struct {
	Content  string                 `json:"content"`
	SenderID string                 `json:"sender_id"`
	ChatID   string                 `json:"chat_id"`
	Metadata map[string]interface{} `json:"metadata,omitempty"`
}

// webhookResponse is the JSON body returned in the HTTP response.
type webhookResponse struct {
	Content string `json:"content"`
	Error   string `json:"error,omitempty"`
}

// webhookPendingRequest tracks a request waiting for its OutboundMessage response.
type webhookPendingRequest struct {
	ch       chan string
	deadline time.Time
}

// WebhookInboundChannel receives HTTP POST requests, publishes them to the
// message bus as InboundMessages, and returns the resulting OutboundMessage
// content as the HTTP response.
type WebhookInboundChannel struct {
	*BaseChannel
	config       WebhookInboundConfig
	httpServer   *http.Server
	ctx          context.Context
	cancel       context.CancelFunc
	pending      sync.Map // chatID -> *webhookPendingRequest
	pendingMu    sync.Mutex
}

// NewWebhookInboundChannel creates a new WebhookInboundChannel.
func NewWebhookInboundChannel(cfg WebhookInboundConfig, messageBus *bus.MessageBus) (*WebhookInboundChannel, error) {
	if cfg.ListenAddr == "" {
		cfg.ListenAddr = ":9090"
	}
	if cfg.Path == "" {
		cfg.Path = "/webhook/incoming"
	}
	if cfg.ChannelName == "" {
		cfg.ChannelName = "webhook"
	}

	base := NewBaseChannel(cfg.ChannelName, cfg, messageBus, cfg.AllowFrom)

	return &WebhookInboundChannel{
		BaseChannel: base,
		config:      cfg,
	}, nil
}

// Start launches the HTTP server for receiving webhook requests.
func (c *WebhookInboundChannel) Start(ctx context.Context) error {
	logger.InfoCF("webhook_inbound", "Starting Webhook Inbound channel", map[string]interface{}{
		"listen_addr": c.config.ListenAddr,
		"path":        c.config.Path,
		"auth":        c.config.APIKey != "",
	})

	c.ctx, c.cancel = context.WithCancel(ctx)

	mux := http.NewServeMux()

	// Register the primary webhook path
	mux.HandleFunc(c.config.Path, c.webhookHandler)

	// Register path-based routing: /webhook/{channel_name}/{chat_id}
	// This pattern allows external systems to route to specific chats.
	routingPath := c.config.Path + "/"
	mux.HandleFunc(routingPath, c.webhookRoutingHandler)

	c.httpServer = &http.Server{
		Addr:    c.config.ListenAddr,
		Handler: mux,
	}

	go func() {
		logger.InfoCF("webhook_inbound", "Webhook server listening", map[string]interface{}{
			"addr": c.config.ListenAddr,
			"path": c.config.Path,
		})
		if err := c.httpServer.ListenAndServe(); err != nil && err != http.ErrServerClosed {
			logger.ErrorCF("webhook_inbound", "Webhook server error", map[string]interface{}{
				"error": err.Error(),
			})
		}
	}()

	// Start cleanup goroutine for stale pending requests
	go c.cleanupPending()

	c.setRunning(true)
	logger.InfoC("webhook_inbound", "Webhook Inbound channel started")
	return nil
}

// Stop gracefully shuts down the HTTP server.
func (c *WebhookInboundChannel) Stop(ctx context.Context) error {
	logger.InfoC("webhook_inbound", "Stopping Webhook Inbound channel")

	if c.cancel != nil {
		c.cancel()
	}

	if c.httpServer != nil {
		shutdownCtx, cancel := context.WithTimeout(ctx, 5*time.Second)
		defer cancel()
		if err := c.httpServer.Shutdown(shutdownCtx); err != nil {
			logger.ErrorCF("webhook_inbound", "Webhook server shutdown error", map[string]interface{}{
				"error": err.Error(),
			})
		}
	}

	// Close all pending request channels
	c.pending.Range(func(key, value interface{}) bool {
		if pr, ok := value.(*webhookPendingRequest); ok {
			close(pr.ch)
		}
		c.pending.Delete(key)
		return true
	})

	c.setRunning(false)
	logger.InfoC("webhook_inbound", "Webhook Inbound channel stopped")
	return nil
}

// Send delivers an OutboundMessage. For the webhook channel, this resolves
// the pending HTTP request that matches the ChatID, writing the response
// content back to the waiting HTTP handler.
func (c *WebhookInboundChannel) Send(ctx context.Context, msg bus.OutboundMessage) error {
	if !c.IsRunning() {
		return fmt.Errorf("webhook inbound channel not running")
	}

	logger.DebugCF("webhook_inbound", "Sending response", map[string]interface{}{
		"chat_id": msg.ChatID,
		"preview": utils.Truncate(msg.Content, 100),
	})

	// Look for a pending request matching this chatID
	if val, ok := c.pending.LoadAndDelete(msg.ChatID); ok {
		pr := val.(*webhookPendingRequest)
		select {
		case pr.ch <- msg.Content:
		default:
			logger.WarnCF("webhook_inbound", "Pending request channel full, dropping response", map[string]interface{}{
				"chat_id": msg.ChatID,
			})
		}
	}

	return nil
}

// webhookHandler processes POST requests on the primary webhook path.
// It reads the JSON body, validates the API key if configured, publishes
// an InboundMessage to the bus, and waits for the OutboundMessage response.
func (c *WebhookInboundChannel) webhookHandler(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost {
		http.Error(w, "Method not allowed", http.StatusMethodNotAllowed)
		return
	}

	// Validate API key if configured
	if c.config.APIKey != "" {
		key := r.Header.Get("X-Webhook-Key")
		if key != c.config.APIKey {
			logger.WarnC("webhook_inbound", "Invalid API key in webhook request")
			http.Error(w, "Forbidden", http.StatusForbidden)
			return
		}
	}

	body, err := io.ReadAll(r.Body)
	if err != nil {
		logger.ErrorCF("webhook_inbound", "Failed to read request body", map[string]interface{}{
			"error": err.Error(),
		})
		http.Error(w, "Bad request", http.StatusBadRequest)
		return
	}
	defer r.Body.Close()

	var req webhookRequest
	if err := json.Unmarshal(body, &req); err != nil {
		logger.ErrorCF("webhook_inbound", "Failed to parse request body", map[string]interface{}{
			"error": err.Error(),
		})
		http.Error(w, "Invalid JSON", http.StatusBadRequest)
		return
	}

	if req.Content == "" {
		http.Error(w, "content field is required", http.StatusBadRequest)
		return
	}

	// Defaults
	if req.SenderID == "" {
		req.SenderID = "webhook"
	}
	if req.ChatID == "" {
		req.ChatID = "webhook:default"
	}

	// Convert metadata from map[string]interface{} to map[string]string
	metadata := make(map[string]string)
	for k, v := range req.Metadata {
		metadata[k] = fmt.Sprintf("%v", v)
	}
	metadata["platform"] = "webhook_inbound"

	logger.DebugCF("webhook_inbound", "Received webhook request", map[string]interface{}{
		"sender_id": req.SenderID,
		"chat_id":   req.ChatID,
		"preview":   utils.Truncate(req.Content, 50),
	})

	// Register a pending request to wait for the outbound response
	responseCh := make(chan string, 1)
	pr := &webhookPendingRequest{
		ch:       responseCh,
		deadline: time.Now().Add(5 * time.Minute),
	}
	c.pending.Store(req.ChatID, pr)

	// Publish to the message bus
	c.HandleMessage(req.SenderID, req.ChatID, req.Content, nil, metadata)

	// Wait for the response with a timeout
	select {
	case responseContent := <-responseCh:
		w.Header().Set("Content-Type", "application/json")
		json.NewEncoder(w).Encode(webhookResponse{
			Content: responseContent,
		})
	case <-time.After(4 * time.Minute):
		// Clean up the pending request
		c.pending.Delete(req.ChatID)
		w.Header().Set("Content-Type", "application/json")
		w.WriteHeader(http.StatusGatewayTimeout)
		json.NewEncoder(w).Encode(webhookResponse{
			Error: "response timeout",
		})
	case <-c.ctx.Done():
		c.pending.Delete(req.ChatID)
		w.Header().Set("Content-Type", "application/json")
		w.WriteHeader(http.StatusServiceUnavailable)
		json.NewEncoder(w).Encode(webhookResponse{
			Error: "service shutting down",
		})
	}
}

// webhookRoutingHandler handles path-based routing requests.
// Pattern: /webhook/{channel_name}/{chat_id}
// The channel_name is extracted and passed as metadata; chat_id from the
// path is used if not provided in the JSON body.
func (c *WebhookInboundChannel) webhookRoutingHandler(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost {
		http.Error(w, "Method not allowed", http.StatusMethodNotAllowed)
		return
	}

	// Validate API key if configured
	if c.config.APIKey != "" {
		key := r.Header.Get("X-Webhook-Key")
		if key != c.config.APIKey {
			logger.WarnC("webhook_inbound", "Invalid API key in webhook routing request")
			http.Error(w, "Forbidden", http.StatusForbidden)
			return
		}
	}

	// Extract path segments after the base path
	basePath := strings.TrimSuffix(c.config.Path, "/")
	remaining := strings.TrimPrefix(r.URL.Path, basePath+"/")
	segments := strings.SplitN(remaining, "/", 2)

	var routedChannel, routedChatID string
	if len(segments) >= 1 && segments[0] != "" {
		routedChannel = segments[0]
	}
	if len(segments) >= 2 && segments[1] != "" {
		routedChatID = segments[1]
	}

	body, err := io.ReadAll(r.Body)
	if err != nil {
		logger.ErrorCF("webhook_inbound", "Failed to read routing request body", map[string]interface{}{
			"error": err.Error(),
		})
		http.Error(w, "Bad request", http.StatusBadRequest)
		return
	}
	defer r.Body.Close()

	var req webhookRequest
	if err := json.Unmarshal(body, &req); err != nil {
		logger.ErrorCF("webhook_inbound", "Failed to parse routing request body", map[string]interface{}{
			"error": err.Error(),
		})
		http.Error(w, "Invalid JSON", http.StatusBadRequest)
		return
	}

	if req.Content == "" {
		http.Error(w, "content field is required", http.StatusBadRequest)
		return
	}

	// Use path-provided values as defaults
	if req.SenderID == "" {
		req.SenderID = "webhook"
	}
	// Override chat_id with the path-provided value if available
	if routedChatID != "" {
		req.ChatID = routedChatID
	}
	if req.ChatID == "" {
		req.ChatID = "webhook:default"
	}

	// Build metadata
	metadata := make(map[string]string)
	for k, v := range req.Metadata {
		metadata[k] = fmt.Sprintf("%v", v)
	}
	metadata["platform"] = "webhook_inbound"
	if routedChannel != "" {
		metadata["routed_channel"] = routedChannel
	}

	logger.DebugCF("webhook_inbound", "Received routed webhook request", map[string]interface{}{
		"sender_id":      req.SenderID,
		"chat_id":        req.ChatID,
		"routed_channel": routedChannel,
		"preview":        utils.Truncate(req.Content, 50),
	})

	// Register a pending request to wait for the outbound response
	responseCh := make(chan string, 1)
	pr := &webhookPendingRequest{
		ch:       responseCh,
		deadline: time.Now().Add(5 * time.Minute),
	}
	c.pending.Store(req.ChatID, pr)

	// Publish to the message bus
	c.HandleMessage(req.SenderID, req.ChatID, req.Content, nil, metadata)

	// Wait for the response
	select {
	case responseContent := <-responseCh:
		w.Header().Set("Content-Type", "application/json")
		json.NewEncoder(w).Encode(webhookResponse{
			Content: responseContent,
		})
	case <-time.After(4 * time.Minute):
		c.pending.Delete(req.ChatID)
		w.Header().Set("Content-Type", "application/json")
		w.WriteHeader(http.StatusGatewayTimeout)
		json.NewEncoder(w).Encode(webhookResponse{
			Error: "response timeout",
		})
	case <-c.ctx.Done():
		c.pending.Delete(req.ChatID)
		w.Header().Set("Content-Type", "application/json")
		w.WriteHeader(http.StatusServiceUnavailable)
		json.NewEncoder(w).Encode(webhookResponse{
			Error: "service shutting down",
		})
	}
}

// cleanupPending periodically removes expired pending requests.
func (c *WebhookInboundChannel) cleanupPending() {
	ticker := time.NewTicker(30 * time.Second)
	defer ticker.Stop()

	for {
		select {
		case <-c.ctx.Done():
			return
		case <-ticker.C:
			now := time.Now()
			c.pending.Range(func(key, value interface{}) bool {
				pr := value.(*webhookPendingRequest)
				if now.After(pr.deadline) {
					c.pending.Delete(key)
					logger.DebugCF("webhook_inbound", "Cleaned up expired pending request", map[string]interface{}{
						"chat_id": key,
					})
				}
				return true
			})
		}
	}
}
