// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
// Mastodon Channel - Mastodon REST API + SSE streaming (pure stdlib HTTP)

package channels

import (
	"bufio"
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

// MastodonConfig configures the Mastodon channel.
type MastodonConfig struct {
	Server      string   // "https://mastodon.social"
	AccessToken string   // OAuth access token
	AccountID   string   // optional, auto-detected via verify_credentials
	ChannelName string   // "mastodon"
	AllowFrom   []string // account allowlist (e.g. "@user@mastodon.social")
}

// MastodonChannel implements the Channel interface for Mastodon using
// the REST API and Server-Sent Events (SSE) for streaming notifications.
type MastodonChannel struct {
	*BaseChannel
	config     MastodonConfig
	ctx        context.Context
	cancel     context.CancelFunc
	httpClient *http.Client
	wg         sync.WaitGroup
	accountID  string
}

// mastodonCredentialResponse is the response from verify_credentials.
type mastodonCredentialResponse struct {
	ID       string `json:"id"`
	Username string `json:"username"`
	Acct     string `json:"acct"`
}

// mastodonNotification represents a Mastodon notification.
type mastodonNotification struct {
	ID       string `json:"id"`
	Type     string `json:"type"` // "mention", "favourite", "reblog", "follow", etc.
	Account  mastodonAccount `json:"account"`
	Status   *mastodonStatus `json:"status,omitempty"`
	CreatedAt string `json:"created_at"`
}

// mastodonAccount represents a Mastodon account.
type mastodonAccount struct {
	ID       string `json:"id"`
	Username string `json:"username"`
	Acct     string `json:"acct"`
	DisplayName string `json:"display_name"`
}

// mastodonStatus represents a Mastodon status (toot).
type mastodonStatus struct {
	ID          string `json:"id"`
	Content     string `json:"content"`
	Visibility  string `json:"visibility"`
	InReplyToID string `json:"in_reply_to_id,omitempty"`
	URI         string `json:"uri"`
}

// mastodonPostRequest is the body for creating a new status.
type mastodonPostRequest struct {
	Status      string `json:"status"`
	InReplyToID string `json:"in_reply_to_id,omitempty"`
	Visibility  string `json:"visibility"`
}

// mastodonPostResponse is the response from creating a status.
type mastodonPostResponse struct {
	ID string `json:"id"`
}

// NewMastodonChannel creates a new Mastodon channel instance.
func NewMastodonChannel(cfg MastodonConfig, messageBus *bus.MessageBus) (*MastodonChannel, error) {
	if cfg.Server == "" || cfg.AccessToken == "" {
		return nil, fmt.Errorf("mastodon server and access_token are required")
	}

	// Apply defaults
	cfg.Server = strings.TrimSuffix(cfg.Server, "/")
	if cfg.ChannelName == "" {
		cfg.ChannelName = "mastodon"
	}

	base := NewBaseChannel(cfg.ChannelName, cfg, messageBus, cfg.AllowFrom)

	return &MastodonChannel{
		BaseChannel: base,
		config:      cfg,
		httpClient: &http.Client{
			Timeout: 30 * time.Second,
		},
	}, nil
}

// Start verifies credentials, resolves account ID, and starts the SSE stream.
func (c *MastodonChannel) Start(ctx context.Context) error {
	logger.InfoCF("mastodon", "Starting Mastodon channel", map[string]interface{}{
		"server": c.config.Server,
	})

	c.ctx, c.cancel = context.WithCancel(ctx)

	// Verify credentials and resolve account ID
	if err := c.verifyCredentials(); err != nil {
		return fmt.Errorf("mastodon credential verification failed: %w", err)
	}

	// Start the SSE streaming loop
	c.wg.Add(1)
	go c.sseLoop()

	c.setRunning(true)
	logger.InfoC("mastodon", "Mastodon channel started")
	return nil
}

// Stop gracefully stops the Mastodon channel.
func (c *MastodonChannel) Stop(ctx context.Context) error {
	logger.InfoC("mastodon", "Stopping Mastodon channel")

	if c.cancel != nil {
		c.cancel()
	}

	c.wg.Wait()
	c.setRunning(false)
	logger.InfoC("mastodon", "Mastodon channel stopped")
	return nil
}

// Send posts a status reply on Mastodon.
func (c *MastodonChannel) Send(ctx context.Context, msg bus.OutboundMessage) error {
	if !c.IsRunning() {
		return fmt.Errorf("mastodon channel not running")
	}

	if msg.ChatID == "" {
		return fmt.Errorf("no status/chat_id specified for reply")
	}

	logger.DebugCF("mastodon", "Posting status reply", map[string]interface{}{
		"in_reply_to": msg.ChatID,
		"preview":     utils.Truncate(msg.Content, 50),
	})

	// Strip HTML from content since Mastodon expects plain text
	content := stripHTMLTags(msg.Content)

	postBody := mastodonPostRequest{
		Status:      content,
		InReplyToID: msg.ChatID,
		Visibility:  "unlisted",
	}

	body, err := json.Marshal(postBody)
	if err != nil {
		return fmt.Errorf("failed to marshal post request: %w", err)
	}

	url := fmt.Sprintf("%s/api/v1/statuses", c.config.Server)
	req, err := http.NewRequestWithContext(ctx, http.MethodPost, url, bytes.NewReader(body))
	if err != nil {
		return fmt.Errorf("failed to create request: %w", err)
	}
	req.Header.Set("Authorization", "Bearer "+c.config.AccessToken)
	req.Header.Set("Content-Type", "application/json")

	resp, err := c.httpClient.Do(req)
	if err != nil {
		return fmt.Errorf("post request failed: %w", err)
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK && resp.StatusCode != http.StatusCreated {
		respBody, _ := io.ReadAll(resp.Body)
		return fmt.Errorf("post returned status %d: %s", resp.StatusCode, string(respBody))
	}

	var postResp mastodonPostResponse
	if err := json.NewDecoder(resp.Body).Decode(&postResp); err != nil {
		logger.WarnCF("mastodon", "Could not decode post response", map[string]interface{}{
			"error": err.Error(),
		})
	}

	logger.DebugCF("mastodon", "Status posted", map[string]interface{}{
		"status_id": postResp.ID,
	})

	return nil
}

// verifyCredentials validates the access token and resolves the account ID.
func (c *MastodonChannel) verifyCredentials() error {
	url := fmt.Sprintf("%s/api/v1/accounts/verify_credentials", c.config.Server)

	req, err := http.NewRequestWithContext(c.ctx, http.MethodGet, url, nil)
	if err != nil {
		return err
	}
	req.Header.Set("Authorization", "Bearer "+c.config.AccessToken)

	resp, err := c.httpClient.Do(req)
	if err != nil {
		return fmt.Errorf("verify request failed: %w", err)
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		body, _ := io.ReadAll(resp.Body)
		return fmt.Errorf("verify returned status %d: %s", resp.StatusCode, string(body))
	}

	var cred mastodonCredentialResponse
	if err := json.NewDecoder(resp.Body).Decode(&cred); err != nil {
		return fmt.Errorf("failed to decode credentials response: %w", err)
	}

	c.accountID = cred.ID

	logger.InfoCF("mastodon", "Credentials verified", map[string]interface{}{
		"account_id": cred.ID,
		"username":   cred.Username,
		"acct":       cred.Acct,
	})

	// Use auto-detected ID if not configured
	if c.config.AccountID == "" {
		c.config.AccountID = cred.ID
	}

	return nil
}

// sseLoop manages the SSE connection with automatic reconnection.
func (c *MastodonChannel) sseLoop() {
	defer c.wg.Done()

	for {
		select {
		case <-c.ctx.Done():
			return
		default:
		}

		if err := c.connectSSE(); err != nil {
			if c.ctx.Err() != nil {
				return
			}

			logger.ErrorCF("mastodon", "SSE connection error", map[string]interface{}{
				"error": err.Error(),
			})

			// Back off before reconnecting
			select {
			case <-c.ctx.Done():
				return
			case <-time.After(10 * time.Second):
				continue
			}
		}
	}
}

// connectSSE establishes an SSE connection and processes events.
func (c *MastodonChannel) connectSSE() error {
	url := fmt.Sprintf("%s/api/v1/streaming/user", c.config.Server)

	req, err := http.NewRequestWithContext(c.ctx, http.MethodGet, url, nil)
	if err != nil {
		return err
	}
	req.Header.Set("Authorization", "Bearer "+c.config.AccessToken)
	req.Header.Set("Accept", "text/event-stream")
	req.Header.Set("Cache-Control", "no-cache")

	// Use a client without timeout for streaming
	streamClient := &http.Client{}
	resp, err := streamClient.Do(req)
	if err != nil {
		return fmt.Errorf("SSE connection failed: %w", err)
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		body, _ := io.ReadAll(resp.Body)
		return fmt.Errorf("SSE returned status %d: %s", resp.StatusCode, string(body))
	}

	logger.DebugC("mastodon", "SSE stream connected")

	// Parse SSE events
	scanner := bufio.NewScanner(resp.Body)
	scanner.Buffer(make([]byte, 0, 64*1024), 1024*1024)

	var eventType, eventData string

	for scanner.Scan() {
		select {
		case <-c.ctx.Done():
			return nil
		default:
		}

		line := scanner.Text()

		if strings.HasPrefix(line, "event: ") {
			eventType = strings.TrimPrefix(line, "event: ")
			continue
		}

		if strings.HasPrefix(line, "data: ") {
			eventData = strings.TrimPrefix(line, "data: ")
			continue
		}

		// Empty line signals end of event
		if line == "" && eventType != "" && eventData != "" {
			c.processSSEEvent(eventType, eventData)
			eventType = ""
			eventData = ""
		}
	}

	if err := scanner.Err(); err != nil {
		if c.ctx.Err() != nil {
			return nil
		}
		return fmt.Errorf("SSE stream read error: %w", err)
	}

	return nil
}

// processSSEEvent handles a single SSE event from the Mastodon streaming API.
func (c *MastodonChannel) processSSEEvent(eventType, data string) {
	switch eventType {
	case "notification":
		c.processNotification(data)
	case "update", "delete", "filters_changed":
		// Ignore non-notification events
	default:
		logger.DebugCF("mastodon", "Ignoring SSE event type", map[string]interface{}{
			"event_type": eventType,
		})
	}
}

// processNotification parses and handles a Mastodon notification.
func (c *MastodonChannel) processNotification(data string) {
	var notif mastodonNotification
	if err := json.Unmarshal([]byte(data), &notif); err != nil {
		logger.ErrorCF("mastodon", "Failed to parse notification", map[string]interface{}{
			"error": err.Error(),
		})
		return
	}

	// Only handle mentions
	if notif.Type != "mention" {
		return
	}

	if notif.Status == nil {
		return
	}

	// Extract text content (strip HTML)
	content := stripHTMLTags(notif.Status.Content)
	if content == "" {
		return
	}

	senderID := notif.Account.Acct
	if senderID == "" {
		senderID = notif.Account.Username
	}

	chatID := notif.Status.ID

	metadata := map[string]string{
		"platform":    "mastodon",
		"sender":      senderID,
		"sender_name": notif.Account.DisplayName,
		"status_id":   notif.Status.ID,
		"visibility":  notif.Status.Visibility,
		"uri":         notif.Status.URI,
	}

	logger.DebugCF("mastodon", "Received mention", map[string]interface{}{
		"sender":    senderID,
		"status_id": notif.Status.ID,
		"preview":   utils.Truncate(content, 50),
	})

	c.HandleMessage(senderID, chatID, content, nil, metadata)
}

// stripHTMLTags removes basic HTML tags from a string.
func stripHTMLTags(s string) string {
	var result strings.Builder
	result.Grow(len(s))

	inTag := false
	for _, ch := range s {
		if ch == '<' {
			inTag = true
			continue
		}
		if ch == '>' {
			inTag = false
			// Add a space to separate words that were in different tags
			result.WriteRune(' ')
			continue
		}
		if !inTag {
			result.WriteRune(ch)
		}
	}

	return strings.TrimSpace(result.String())
}

// Ensure MastodonChannel implements the Channel interface at compile time.
var _ Channel = (*MastodonChannel)(nil)
