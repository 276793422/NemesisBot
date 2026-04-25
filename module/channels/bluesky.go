// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
// Bluesky Channel - AT Protocol REST API (pure stdlib HTTP)

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

// BlueskyConfig configures the Bluesky channel.
type BlueskyConfig struct {
	Server      string   // "https://bsky.social"
	Handle      string   // "nemesisbot.bsky.social"
	Password    string   // App Password
	DID         string   // optional, auto-resolved via createSession
	ChannelName string   // "bluesky"
	AllowFrom   []string // handle allowlist
	PollInterval int     // seconds between notification polls (default: 10)
}

// BlueskyChannel implements the Channel interface for Bluesky (AT Protocol)
// using the REST API with polling for notifications.
type BlueskyChannel struct {
	*BaseChannel
	config      BlueskyConfig
	ctx         context.Context
	cancel      context.CancelFunc
	httpClient  *http.Client
	wg          sync.WaitGroup
	accessToken string
	did         string
	handle      string
	seenNotifs  map[string]bool
	seenMu      sync.Mutex
}

// blueskySessionResponse is the response from com.atproto.server.createSession.
type blueskySessionResponse struct {
	Did        string `json:"did"`
	Handle     string `json:"handle"`
	Email      string `json:"email,omitempty"`
	AccessJwt  string `json:"accessJwt"`
	RefreshJwt string `json:"refreshJwt"`
	Active     bool   `json:"active"`
}

// blueskyNotificationsResponse is the response from listNotifications.
type blueskyNotificationsResponse struct {
	Notifications []blueskyNotification `json:"notifications"`
	Cursor        string                `json:"cursor,omitempty"`
}

// blueskyNotification represents a single Bluesky notification.
type blueskyNotification struct {
	ID        string `json:"id"`
	Reason    string `json:"reason"` // "mention", "reply", "quote", "like", "repost", "follow"
	Author    blueskyActor `json:"author"`
	Record    json.RawMessage `json:"record,omitempty"`
	IsRead    bool   `json:"isRead"`
	IndexedAt string `json:"indexedAt"`
}

// blueskyActor represents a Bluesky account.
type blueskyActor struct {
	Did    string `json:"did"`
	Handle string `json:"handle"`
	DisplayName string `json:"displayName,omitempty"`
}

// blueskyPostRecord represents a post record in AT Protocol.
type blueskyPostRecord struct {
	Type      string `json:"$type"`
	Text      string `json:"text"`
	CreatedAt string `json:"createdAt"`
	Reply     *blueskyReplyRef `json:"reply,omitempty"`
}

// blueskyReplyRef references the parent and root posts for a reply.
type blueskyReplyRef struct {
	Root   blueskyStrongRef `json:"root"`
	Parent blueskyStrongRef `json:"parent"`
}

// blueskyStrongRef is a strong reference to a record.
type blueskyStrongRef struct {
	URI string `json:"uri"`
	CID string `json:"cid"`
}

// blueskyCreateRecordRequest is the body for creating a record.
type blueskyCreateRecordRequest struct {
	Repo       string      `json:"repo"`
	Collection string      `json:"collection"`
	Record     interface{} `json:"record"`
}

// blueskyCreateRecordResponse is the response from creating a record.
type blueskyCreateRecordResponse struct {
	URI string `json:"uri"`
	CID string `json:"cid"`
}

// blueskyGetRecordResponse is the response from getting a record.
type blueskyGetRecordResponse struct {
	URI   string          `json:"uri"`
	CID   string          `json:"cid"`
	Value json.RawMessage `json:"value"`
}

// NewBlueskyChannel creates a new Bluesky channel instance.
func NewBlueskyChannel(cfg BlueskyConfig, messageBus *bus.MessageBus) (*BlueskyChannel, error) {
	if cfg.Server == "" || cfg.Handle == "" || cfg.Password == "" {
		return nil, fmt.Errorf("bluesky server, handle, and password are required")
	}

	// Apply defaults
	cfg.Server = strings.TrimSuffix(cfg.Server, "/")
	if cfg.ChannelName == "" {
		cfg.ChannelName = "bluesky"
	}
	if cfg.PollInterval == 0 {
		cfg.PollInterval = 10
	}

	base := NewBaseChannel(cfg.ChannelName, cfg, messageBus, cfg.AllowFrom)

	return &BlueskyChannel{
		BaseChannel: base,
		config:      cfg,
		httpClient: &http.Client{
			Timeout: 30 * time.Second,
		},
		seenNotifs: make(map[string]bool),
	}, nil
}

// Start creates a session and begins polling for notifications.
func (c *BlueskyChannel) Start(ctx context.Context) error {
	logger.InfoCF("bluesky", "Starting Bluesky channel", map[string]interface{}{
		"server":        c.config.Server,
		"handle":        c.config.Handle,
		"poll_interval": c.config.PollInterval,
	})

	c.ctx, c.cancel = context.WithCancel(ctx)

	// Create session to get access token and DID
	if err := c.createSession(); err != nil {
		return fmt.Errorf("bluesky session creation failed: %w", err)
	}

	// Start notification polling loop
	c.wg.Add(1)
	go c.pollLoop()

	c.setRunning(true)
	logger.InfoC("bluesky", "Bluesky channel started")
	return nil
}

// Stop gracefully stops the Bluesky channel.
func (c *BlueskyChannel) Stop(ctx context.Context) error {
	logger.InfoC("bluesky", "Stopping Bluesky channel")

	if c.cancel != nil {
		c.cancel()
	}

	c.wg.Wait()
	c.setRunning(false)
	logger.InfoC("bluesky", "Bluesky channel stopped")
	return nil
}

// Send posts a reply on Bluesky.
func (c *BlueskyChannel) Send(ctx context.Context, msg bus.OutboundMessage) error {
	if !c.IsRunning() {
		return fmt.Errorf("bluesky channel not running")
	}

	if msg.ChatID == "" {
		return fmt.Errorf("no post URI specified in chat_id")
	}

	logger.DebugCF("bluesky", "Posting reply", map[string]interface{}{
		"reply_to": msg.ChatID,
		"preview":  utils.Truncate(msg.Content, 50),
	})

	// Resolve the parent post's CID
	parentCID, err := c.resolveRecordCID(ctx, msg.ChatID)
	if err != nil {
		return fmt.Errorf("failed to resolve parent post CID: %w", err)
	}

	// Determine root: if the parent is itself a reply, we need the root.
	// For simplicity, treat the parent as the root (works for single-level replies).
	rootURI := msg.ChatID
	rootCID := parentCID

	now := time.Now().UTC().Format(time.RFC3339Nano)

	record := blueskyPostRecord{
		Type:      "app.bsky.feed.post",
		Text:      msg.Content,
		CreatedAt: now,
		Reply: &blueskyReplyRef{
			Root: blueskyStrongRef{
				URI: rootURI,
				CID: rootCID,
			},
			Parent: blueskyStrongRef{
				URI: msg.ChatID,
				CID: parentCID,
			},
		},
	}

	createReq := blueskyCreateRecordRequest{
		Repo:       c.did,
		Collection: "app.bsky.feed.post",
		Record:     record,
	}

	body, err := json.Marshal(createReq)
	if err != nil {
		return fmt.Errorf("failed to marshal create request: %w", err)
	}

	url := fmt.Sprintf("%s/xrpc/com.atproto.repo.createRecord", c.config.Server)
	req, err := http.NewRequestWithContext(ctx, http.MethodPost, url, bytes.NewReader(body))
	if err != nil {
		return fmt.Errorf("failed to create request: %w", err)
	}
	req.Header.Set("Authorization", "Bearer "+c.accessToken)
	req.Header.Set("Content-Type", "application/json")

	resp, err := c.httpClient.Do(req)
	if err != nil {
		return fmt.Errorf("create record request failed: %w", err)
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK && resp.StatusCode != http.StatusCreated {
		respBody, _ := io.ReadAll(resp.Body)
		return fmt.Errorf("create record returned status %d: %s", resp.StatusCode, string(respBody))
	}

	var createResp blueskyCreateRecordResponse
	if err := json.NewDecoder(resp.Body).Decode(&createResp); err != nil {
		logger.WarnCF("bluesky", "Could not decode create response", map[string]interface{}{
			"error": err.Error(),
		})
	}

	logger.DebugCF("bluesky", "Reply posted", map[string]interface{}{
		"uri": createResp.URI,
	})

	return nil
}

// createSession authenticates with the Bluesky server and obtains an access token.
func (c *BlueskyChannel) createSession() error {
	url := fmt.Sprintf("%s/xrpc/com.atproto.server.createSession", c.config.Server)

	payload := map[string]string{
		"identifier": c.config.Handle,
		"password":   c.config.Password,
	}

	body, err := json.Marshal(payload)
	if err != nil {
		return fmt.Errorf("failed to marshal session request: %w", err)
	}

	ctx, cancel := context.WithTimeout(c.ctx, 15*time.Second)
	defer cancel()

	req, err := http.NewRequestWithContext(ctx, http.MethodPost, url, bytes.NewReader(body))
	if err != nil {
		return err
	}
	req.Header.Set("Content-Type", "application/json")

	resp, err := c.httpClient.Do(req)
	if err != nil {
		return fmt.Errorf("session request failed: %w", err)
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		respBody, _ := io.ReadAll(resp.Body)
		return fmt.Errorf("createSession returned status %d: %s", resp.StatusCode, string(respBody))
	}

	var session blueskySessionResponse
	if err := json.NewDecoder(resp.Body).Decode(&session); err != nil {
		return fmt.Errorf("failed to decode session response: %w", err)
	}

	c.accessToken = session.AccessJwt
	c.did = session.Did
	c.handle = session.Handle

	// Use configured DID if provided
	if c.config.DID != "" {
		c.did = c.config.DID
	}

	logger.InfoCF("bluesky", "Session created", map[string]interface{}{
		"did":    c.did,
		"handle": c.handle,
	})

	return nil
}

// pollLoop periodically checks for new notifications.
func (c *BlueskyChannel) pollLoop() {
	defer c.wg.Done()

	ticker := time.NewTicker(time.Duration(c.config.PollInterval) * time.Second)
	defer ticker.Stop()

	// Poll immediately on start
	c.pollNotifications()

	for {
		select {
		case <-c.ctx.Done():
			return
		case <-ticker.C:
			c.pollNotifications()
		}
	}
}

// pollNotifications fetches and processes new notifications.
func (c *BlueskyChannel) pollNotifications() {
	url := fmt.Sprintf("%s/xrpc/app.bsky.notification.listNotifications?limit=50", c.config.Server)

	ctx, cancel := context.WithTimeout(c.ctx, 25*time.Second)
	defer cancel()

	req, err := http.NewRequestWithContext(ctx, http.MethodGet, url, nil)
	if err != nil {
		if c.ctx.Err() == nil {
			logger.ErrorCF("bluesky", "Failed to create notification request", map[string]interface{}{
				"error": err.Error(),
			})
		}
		return
	}
	req.Header.Set("Authorization", "Bearer "+c.accessToken)

	resp, err := c.httpClient.Do(req)
	if err != nil {
		if c.ctx.Err() == nil {
			logger.ErrorCF("bluesky", "Notification request failed", map[string]interface{}{
				"error": err.Error(),
			})
		}
		return
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		respBody, _ := io.ReadAll(resp.Body)
		// If unauthorized, try to refresh session
		if resp.StatusCode == http.StatusUnauthorized || resp.StatusCode == http.StatusForbidden {
			logger.WarnC("bluesky", "Session expired, attempting re-authentication")
			if sessionErr := c.createSession(); sessionErr != nil {
				logger.ErrorCF("bluesky", "Re-authentication failed", map[string]interface{}{
					"error": sessionErr.Error(),
				})
			}
			return
		}
		logger.ErrorCF("bluesky", "Notification poll returned non-OK", map[string]interface{}{
			"status": resp.StatusCode,
			"body":   string(respBody),
		})
		return
	}

	var notifsResp blueskyNotificationsResponse
	if err := json.NewDecoder(resp.Body).Decode(&notifsResp); err != nil {
		logger.ErrorCF("bluesky", "Failed to decode notifications", map[string]interface{}{
			"error": err.Error(),
		})
		return
	}

	for i := range notifsResp.Notifications {
		c.processNotification(&notifsResp.Notifications[i])
	}

	// Mark notifications as read
	if len(notifsResp.Notifications) > 0 {
		c.updateSeen(notifsResp.Notifications[len(notifsResp.Notifications)-1].IndexedAt)
	}
}

// processNotification handles a single Bluesky notification.
func (c *BlueskyChannel) processNotification(notif *blueskyNotification) {
	// Only handle mentions and replies
	if notif.Reason != "mention" && notif.Reason != "reply" {
		return
	}

	// Skip already-seen notifications
	c.seenMu.Lock()
	if c.seenNotifs[notif.ID] {
		c.seenMu.Unlock()
		return
	}
	c.seenNotifs[notif.ID] = true

	// Cap the seen cache size
	if len(c.seenNotifs) > 500 {
		c.seenNotifs = make(map[string]bool)
		c.seenNotifs[notif.ID] = true
	}
	c.seenMu.Unlock()

	// Extract text from the record
	var postRecord blueskyPostRecord
	if err := json.Unmarshal(notif.Record, &postRecord); err != nil {
		logger.ErrorCF("bluesky", "Failed to parse post record", map[string]interface{}{
			"error": err.Error(),
		})
		return
	}

	if postRecord.Text == "" {
		return
	}

	// Build the AT URI for the post (used as chatID for replies)
	// Format: at://did:plc:xxx/app.bsky.feed.post/xxx
	postURI := fmt.Sprintf("at://%s/app.bsky.feed.post/%s", notif.Author.Did, notif.ID)

	senderID := notif.Author.Handle
	chatID := postURI

	metadata := map[string]string{
		"platform":    "bluesky",
		"sender_did":  notif.Author.Did,
		"sender":      senderID,
		"sender_name": notif.Author.DisplayName,
		"reason":      notif.Reason,
		"notif_id":    notif.ID,
	}

	logger.DebugCF("bluesky", "Received notification", map[string]interface{}{
		"sender":  senderID,
		"reason":  notif.Reason,
		"preview": utils.Truncate(postRecord.Text, 50),
	})

	c.HandleMessage(senderID, chatID, postRecord.Text, nil, metadata)
}

// updateSeen marks notifications as read via the updateSeen endpoint.
func (c *BlueskyChannel) updateSeen(indexedAt string) {
	url := fmt.Sprintf("%s/xrpc/app.bsky.notification.updateSeen", c.config.Server)

	payload := map[string]string{
		"seenAt": indexedAt,
	}

	body, err := json.Marshal(payload)
	if err != nil {
		return
	}

	ctx, cancel := context.WithTimeout(c.ctx, 10*time.Second)
	defer cancel()

	req, err := http.NewRequestWithContext(ctx, http.MethodPost, url, bytes.NewReader(body))
	if err != nil {
		return
	}
	req.Header.Set("Authorization", "Bearer "+c.accessToken)
	req.Header.Set("Content-Type", "application/json")

	resp, err := c.httpClient.Do(req)
	if err != nil {
		return
	}
	resp.Body.Close()
}

// resolveRecordCID fetches a record's CID from the AT Protocol.
func (c *BlueskyChannel) resolveRecordCID(ctx context.Context, atURI string) (string, error) {
	// Parse the AT URI: at://did:plc:xxx/app.bsky.feed.post/xxx
	uri := atURI
	if strings.HasPrefix(uri, "at://") {
		uri = strings.TrimPrefix(uri, "at://")
	}

	parts := strings.SplitN(uri, "/", 3)
	if len(parts) < 3 {
		return "", fmt.Errorf("invalid AT URI: %s", atURI)
	}

	repo := parts[0]
	collection := parts[1]
	rkey := parts[2]

	url := fmt.Sprintf("%s/xrpc/com.atproto.repo.getRecord?repo=%s&collection=%s&rkey=%s",
		c.config.Server, repo, collection, rkey)

	req, err := http.NewRequestWithContext(ctx, http.MethodGet, url, nil)
	if err != nil {
		return "", err
	}
	req.Header.Set("Authorization", "Bearer "+c.accessToken)

	resp, err := c.httpClient.Do(req)
	if err != nil {
		return "", fmt.Errorf("getRecord request failed: %w", err)
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		respBody, _ := io.ReadAll(resp.Body)
		return "", fmt.Errorf("getRecord returned status %d: %s", resp.StatusCode, string(respBody))
	}

	var recordResp blueskyGetRecordResponse
	if err := json.NewDecoder(resp.Body).Decode(&recordResp); err != nil {
		return "", fmt.Errorf("failed to decode getRecord response: %w", err)
	}

	return recordResp.CID, nil
}

// Ensure BlueskyChannel implements the Channel interface at compile time.
var _ Channel = (*BlueskyChannel)(nil)
