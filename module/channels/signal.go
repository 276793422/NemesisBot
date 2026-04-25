// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
// Signal Channel - signal-cli-rest-api REST interface (pure stdlib HTTP)

package channels

import (
	"bytes"
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

// SignalConfig configures the Signal channel.
// Requires an external signal-cli-rest-api instance (typically via Docker).
type SignalConfig struct {
	APIURL       string   // "http://localhost:8080"
	PhoneNumber  string   // "+1234567890"
	ChannelName  string   // "signal"
	AllowFrom    []string // sender phone number allowlist
	PollInterval int      // seconds between receive polls (default: 5)
}

// SignalChannel implements the Channel interface for Signal messaging
// via the signal-cli-rest-api REST interface.
type SignalChannel struct {
	*BaseChannel
	config     SignalConfig
	ctx        context.Context
	cancel     context.CancelFunc
	httpClient *http.Client
	wg         sync.WaitGroup
}

// signalReceiveResponse is the JSON response from the receive endpoint.
type signalReceiveResponse struct {
	Envelope []signalEnvelope `json:"envelope,omitempty"`
}

// signalEnvelope represents a Signal message envelope.
type signalEnvelope struct {
	Source          string `json:"source"`
	SourceNumber    string `json:"sourceNumber"`
	SourceName      string `json:"sourceName"`
	SourceDevice    int    `json:"sourceDevice"`
	Timestamp       int64  `json:"timestamp"`
	DataMessage     *signalDataMessage     `json:"dataMessage,omitempty"`
	SyncMessage     *signalSyncMessage     `json:"syncMessage,omitempty"`
	ReceiptMessage  *signalReceiptMessage  `json:"receiptMessage,omitempty"`
	TypingMessage   *signalTypingMessage   `json:"typingMessage,omitempty"`
}

// signalDataMessage represents the content of a Signal data message.
type signalDataMessage struct {
	Timestamp        int64  `json:"timestamp"`
	Message          string `json:"message"`
	GroupInfo        *signalGroupInfo `json:"groupInfo,omitempty"`
}

// signalGroupInfo contains group metadata.
type signalGroupInfo struct {
	GroupID string `json:"groupId"`
	Name    string `json:"name"`
}

// signalSyncMessage represents a sync message from another device.
type signalSyncMessage struct {
	SentMessage *signalDataMessage `json:"sentMessage,omitempty"`
}

// signalReceiptMessage represents a receipt (read/delivery).
type signalReceiptMessage struct {
	Type         string  `json:"type"`
	Timestamps   []int64 `json:"timestamps,omitempty"`
}

// signalTypingMessage represents a typing indicator.
type signalTypingMessage struct {
	Action    string `json:"action"`
	Timestamp int64  `json:"timestamp"`
}

// signalSendRequest is the JSON body for sending a message.
type signalSendRequest struct {
	Message     string `json:"message"`
	Number      string `json:"number,omitempty"`
	Recipients  []string `json:"recipients,omitempty"`
	GroupID     string `json:"groupId,omitempty"`
}

// signalAboutResponse is the response from the /v1/about endpoint.
type signalAboutResponse struct {
	APIVersions []string `json:"versions"`
}

// NewSignalChannel creates a new Signal channel instance.
func NewSignalChannel(cfg SignalConfig, messageBus *bus.MessageBus) (*SignalChannel, error) {
	if cfg.APIURL == "" {
		return nil, fmt.Errorf("signal api_url is required")
	}
	if cfg.PhoneNumber == "" {
		return nil, fmt.Errorf("signal phone_number is required")
	}

	// Apply defaults
	cfg.APIURL = strings.TrimSuffix(cfg.APIURL, "/")
	if cfg.ChannelName == "" {
		cfg.ChannelName = "signal"
	}
	if cfg.PollInterval == 0 {
		cfg.PollInterval = 5
	}

	base := NewBaseChannel(cfg.ChannelName, cfg, messageBus, cfg.AllowFrom)

	return &SignalChannel{
		BaseChannel: base,
		config:      cfg,
		httpClient: &http.Client{
			Timeout: 120 * time.Second, // Long timeout for receive polling
		},
	}, nil
}

// Start verifies the API and begins the receive polling loop.
func (c *SignalChannel) Start(ctx context.Context) error {
	logger.InfoCF("signal", "Starting Signal channel", map[string]interface{}{
		"api_url":       c.config.APIURL,
		"phone_number":  c.config.PhoneNumber,
		"poll_interval": c.config.PollInterval,
	})

	c.ctx, c.cancel = context.WithCancel(ctx)

	// Verify API availability
	if err := c.verifyAPI(); err != nil {
		return fmt.Errorf("signal API verification failed: %w", err)
	}

	// Start receive polling loop
	c.wg.Add(1)
	go c.receiveLoop()

	c.setRunning(true)
	logger.InfoC("signal", "Signal channel started")
	return nil
}

// Stop gracefully stops the Signal channel.
func (c *SignalChannel) Stop(ctx context.Context) error {
	logger.InfoC("signal", "Stopping Signal channel")

	if c.cancel != nil {
		c.cancel()
	}

	c.wg.Wait()
	c.setRunning(false)
	logger.InfoC("signal", "Signal channel stopped")
	return nil
}

// Send sends a text message via the signal-cli-rest-api.
func (c *SignalChannel) Send(ctx context.Context, msg bus.OutboundMessage) error {
	if !c.IsRunning() {
		return fmt.Errorf("signal channel not running")
	}

	if msg.ChatID == "" {
		return fmt.Errorf("no recipient specified in chat_id")
	}

	logger.DebugCF("signal", "Sending message", map[string]interface{}{
		"recipient": msg.ChatID,
		"preview":   utils.Truncate(msg.Content, 50),
	})

	// Determine if this is a group or individual message
	reqBody := signalSendRequest{
		Message: msg.Content,
	}

	if strings.HasPrefix(msg.ChatID, "group:") {
		reqBody.GroupID = strings.TrimPrefix(msg.ChatID, "group:")
	} else {
		reqBody.Number = msg.ChatID
	}

	body, err := json.Marshal(reqBody)
	if err != nil {
		return fmt.Errorf("failed to marshal send request: %w", err)
	}

	url := fmt.Sprintf("%s/v2/send", c.config.APIURL)
	req, err := http.NewRequestWithContext(ctx, http.MethodPost, url, bytes.NewReader(body))
	if err != nil {
		return fmt.Errorf("failed to create request: %w", err)
	}
	req.Header.Set("Content-Type", "application/json")

	// Use phone number as the account identifier in the query parameter
	q := req.URL.Query()
	q.Set("number", c.config.PhoneNumber)
	req.URL.RawQuery = q.Encode()

	resp, err := c.httpClient.Do(req)
	if err != nil {
		return fmt.Errorf("send request failed: %w", err)
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK && resp.StatusCode != http.StatusCreated {
		respBody, _ := io.ReadAll(resp.Body)
		return fmt.Errorf("send returned status %d: %s", resp.StatusCode, string(respBody))
	}

	logger.DebugCF("signal", "Message sent", map[string]interface{}{
		"recipient": msg.ChatID,
	})

	return nil
}

// verifyAPI checks that the signal-cli-rest-api is reachable.
func (c *SignalChannel) verifyAPI() error {
	url := fmt.Sprintf("%s/v1/about", c.config.APIURL)

	ctx, cancel := context.WithTimeout(c.ctx, 10*time.Second)
	defer cancel()

	req, err := http.NewRequestWithContext(ctx, http.MethodGet, url, nil)
	if err != nil {
		return err
	}

	resp, err := c.httpClient.Do(req)
	if err != nil {
		return fmt.Errorf("API unreachable at %s: %w", c.config.APIURL, err)
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		return fmt.Errorf("API returned status %d", resp.StatusCode)
	}

	var about signalAboutResponse
	if err := json.NewDecoder(resp.Body).Decode(&about); err != nil {
		logger.WarnCF("signal", "Could not parse about response", map[string]interface{}{
			"error": err.Error(),
		})
		// Non-fatal: API is reachable
	}

	logger.InfoCF("signal", "API verified", map[string]interface{}{
		"api_url": c.config.APIURL,
	})

	return nil
}

// receiveLoop polls the signal-cli-rest-api for new messages.
func (c *SignalChannel) receiveLoop() {
	defer c.wg.Done()

	ticker := time.NewTicker(time.Duration(c.config.PollInterval) * time.Second)
	defer ticker.Stop()

	// Poll immediately on start
	c.pollMessages()

	for {
		select {
		case <-c.ctx.Done():
			return
		case <-ticker.C:
			c.pollMessages()
		}
	}
}

// pollMessages fetches and processes new messages from the receive endpoint.
func (c *SignalChannel) pollMessages() {
	url := fmt.Sprintf("%s/v1/receive/%s", c.config.APIURL, c.config.PhoneNumber)

	ctx, cancel := context.WithTimeout(c.ctx, 110*time.Second)
	defer cancel()

	req, err := http.NewRequestWithContext(ctx, http.MethodGet, url, nil)
	if err != nil {
		if c.ctx.Err() == nil {
			logger.ErrorCF("signal", "Failed to create receive request", map[string]interface{}{
				"error": err.Error(),
			})
		}
		return
	}

	resp, err := c.httpClient.Do(req)
	if err != nil {
		if c.ctx.Err() == nil {
			logger.ErrorCF("signal", "Receive request failed", map[string]interface{}{
				"error": err.Error(),
			})
		}
		return
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		respBody, _ := io.ReadAll(resp.Body)
		logger.ErrorCF("signal", "Receive returned non-OK status", map[string]interface{}{
			"status": resp.StatusCode,
			"body":   string(respBody),
		})
		return
	}

	// The response is a JSON array of envelopes
	var envelopes []signalEnvelope
	if err := json.NewDecoder(resp.Body).Decode(&envelopes); err != nil {
		// Empty or non-JSON responses are normal when no messages
		return
	}

	for i := range envelopes {
		c.processEnvelope(&envelopes[i])
	}
}

// processEnvelope handles a single Signal message envelope.
func (c *SignalChannel) processEnvelope(envelope *signalEnvelope) {
	// Skip receipts and typing indicators
	if envelope.ReceiptMessage != nil || envelope.TypingMessage != nil {
		return
	}

	// Extract data message
	var dataMsg *signalDataMessage
	if envelope.DataMessage != nil {
		dataMsg = envelope.DataMessage
	} else if envelope.SyncMessage != nil && envelope.SyncMessage.SentMessage != nil {
		// Sync messages are from our own other devices — skip them
		return
	}

	if dataMsg == nil {
		return
	}

	// Skip empty messages
	if dataMsg.Message == "" {
		return
	}

	sender := envelope.SourceNumber
	if sender == "" {
		sender = envelope.Source
	}

	// Determine chatID
	chatID := sender
	if dataMsg.GroupInfo != nil && dataMsg.GroupInfo.GroupID != "" {
		chatID = "group:" + dataMsg.GroupInfo.GroupID
	}

	metadata := map[string]string{
		"platform":  "signal",
		"sender":    sender,
		"sender_name": envelope.SourceName,
		"timestamp": fmt.Sprintf("%d", dataMsg.Timestamp),
	}

	if dataMsg.GroupInfo != nil {
		metadata["group_id"] = dataMsg.GroupInfo.GroupID
		metadata["group_name"] = dataMsg.GroupInfo.Name
	}

	logger.DebugCF("signal", "Received message", map[string]interface{}{
		"sender":  sender,
		"chat_id": chatID,
		"preview": utils.Truncate(dataMsg.Message, 50),
	})

	c.HandleMessage(sender, chatID, dataMsg.Message, nil, metadata)
}

// Ensure SignalChannel implements the Channel interface at compile time.
var _ Channel = (*SignalChannel)(nil)
