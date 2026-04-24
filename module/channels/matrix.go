// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
// Matrix Channel - uses Matrix Client-Server API via long-poll /sync

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

// MatrixConfig configures the Matrix channel.
type MatrixConfig struct {
	Homeserver  string   // "https://matrix.org"
	UserID      string   // "@bot:matrix.org"
	AccessToken string   // Matrix access token
	RoomID      string   // optional default room
	ChannelName string   // "matrix"
	AllowFrom   []string // sender allowlist
}

// MatrixChannel implements the Channel interface for Matrix protocol
// using the Matrix Client-Server API with long-poll /sync for receiving
// messages and /rooms/{roomId}/send/m.room.message for sending.
type MatrixChannel struct {
	*BaseChannel
	config      MatrixConfig
	ctx         context.Context
	cancel      context.CancelFunc
	httpClient  *http.Client
	sinceToken  string       // next_batch token for incremental sync
	syncMu      sync.Mutex   // protects sinceToken
	wg          sync.WaitGroup
	roomMembers sync.Map // roomID -> map[userID]displayName (cached)
}

// Matrix sync API response structures (simplified).

type matrixSyncResponse struct {
	NextBatch string `json:"next_batch"`
	Rooms     struct {
		Join map[string]struct {
			Timeline struct {
				Events []matrixEvent `json:"events"`
			} `json:"timeline"`
		} `json:"join"`
	} `json:"rooms"`
}

type matrixEvent struct {
	Type     string          `json:"type"`
	Content  matrixContent   `json:"content"`
	Sender   string          `json:"sender"`
	EventID  string          `json:"event_id"`
	RoomID   string          `json:"room_id,omitempty"`
	OriginTs int64           `json:"origin_server_ts"`
	StateKey string          `json:"state_key,omitempty"`
	Unsigned json.RawMessage `json:"unsigned,omitempty"`
}

type matrixContent struct {
	MsgType string `json:"msgtype"`
	Body    string `json:"body"`
}

// matrixSendResponse is the response from sending a message event.
type matrixSendResponse struct {
	EventID string `json:"event_id"`
}

// matrixError represents a Matrix API error response.
type matrixError struct {
	ErrCode string `json:"errcode"`
	Error   string `json:"error"`
}

// NewMatrixChannel creates a new Matrix channel instance.
func NewMatrixChannel(cfg MatrixConfig, messageBus *bus.MessageBus) (*MatrixChannel, error) {
	if cfg.Homeserver == "" || cfg.AccessToken == "" {
		return nil, fmt.Errorf("matrix homeserver and access_token are required")
	}

	// Apply defaults
	cfg.Homeserver = strings.TrimSuffix(cfg.Homeserver, "/")
	if cfg.ChannelName == "" {
		cfg.ChannelName = "matrix"
	}

	base := NewBaseChannel(cfg.ChannelName, cfg, messageBus, cfg.AllowFrom)

	return &MatrixChannel{
		BaseChannel: base,
		config:      cfg,
		httpClient: &http.Client{
			Timeout: 120 * time.Second, // Long timeout for long-polling
		},
	}, nil
}

// Start begins the Matrix sync loop using long-polling.
func (c *MatrixChannel) Start(ctx context.Context) error {
	logger.InfoCF("matrix", "Starting Matrix channel", map[string]interface{}{
		"homeserver": c.config.Homeserver,
		"user_id":    c.config.UserID,
		"room_id":    c.config.RoomID,
	})

	c.ctx, c.cancel = context.WithCancel(ctx)

	// Verify the access token by calling /account/whoami
	if err := c.verifyCredentials(); err != nil {
		return fmt.Errorf("matrix credential verification failed: %w", err)
	}

	// Perform an initial sync to get the next_batch token without processing old messages
	if err := c.initialSync(); err != nil {
		logger.WarnCF("matrix", "Initial sync failed, starting fresh", map[string]interface{}{
			"error": err.Error(),
		})
	}

	// Start the sync loop
	c.wg.Add(1)
	go c.syncLoop()

	c.setRunning(true)
	logger.InfoC("matrix", "Matrix channel started")
	return nil
}

// Stop gracefully stops the Matrix channel.
func (c *MatrixChannel) Stop(ctx context.Context) error {
	logger.InfoC("matrix", "Stopping Matrix channel")

	if c.cancel != nil {
		c.cancel()
	}

	c.wg.Wait()

	c.setRunning(false)
	logger.InfoC("matrix", "Matrix channel stopped")
	return nil
}

// Send sends a message to a Matrix room using the Client-Server API.
// The OutboundMessage.ChatID should be the room ID.
func (c *MatrixChannel) Send(ctx context.Context, msg bus.OutboundMessage) error {
	if !c.IsRunning() {
		return fmt.Errorf("matrix channel not running")
	}

	roomID := msg.ChatID
	if roomID == "" {
		// Fall back to default room
		roomID = c.config.RoomID
	}
	if roomID == "" {
		return fmt.Errorf("no room ID specified and no default room configured")
	}

	logger.DebugCF("matrix", "Sending message to room", map[string]interface{}{
		"room_id": roomID,
		"preview": utils.Truncate(msg.Content, 50),
	})

	eventID, err := c.sendMessage(ctx, roomID, msg.Content)
	if err != nil {
		return fmt.Errorf("failed to send matrix message: %w", err)
	}

	logger.DebugCF("matrix", "Message sent", map[string]interface{}{
		"room_id":  roomID,
		"event_id": eventID,
	})

	return nil
}

// verifyCredentials checks that the access token is valid by calling /account/whoami.
func (c *MatrixChannel) verifyCredentials() error {
	url := fmt.Sprintf("%s/_matrix/client/v3/account/whoami", c.config.Homeserver)

	req, err := http.NewRequestWithContext(c.ctx, http.MethodGet, url, nil)
	if err != nil {
		return err
	}
	req.Header.Set("Authorization", "Bearer "+c.config.AccessToken)

	resp, err := c.httpClient.Do(req)
	if err != nil {
		return fmt.Errorf("whoami request failed: %w", err)
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		body, _ := io.ReadAll(resp.Body)
		return fmt.Errorf("whoami returned status %d: %s", resp.StatusCode, string(body))
	}

	var result struct {
		UserID string `json:"user_id"`
	}
	if err := json.NewDecoder(resp.Body).Decode(&result); err != nil {
		return fmt.Errorf("failed to decode whoami response: %w", err)
	}

	logger.InfoCF("matrix", "Credentials verified", map[string]interface{}{
		"user_id": result.UserID,
	})

	// Use the returned user_id if not configured
	if c.config.UserID == "" {
		c.config.UserID = result.UserID
	}

	return nil
}

// initialSync performs an initial sync to get the next_batch token without
// processing any historical messages.
func (c *MatrixChannel) initialSync() error {
	url := fmt.Sprintf("%s/_matrix/client/v3/sync?timeout=0&filter={\"room\":{\"timeline\":{\"limit\":0}}}",
		c.config.Homeserver)

	req, err := http.NewRequestWithContext(c.ctx, http.MethodGet, url, nil)
	if err != nil {
		return err
	}
	req.Header.Set("Authorization", "Bearer "+c.config.AccessToken)

	resp, err := c.httpClient.Do(req)
	if err != nil {
		return err
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		body, _ := io.ReadAll(resp.Body)
		return fmt.Errorf("initial sync returned status %d: %s", resp.StatusCode, string(body))
	}

	var syncResp matrixSyncResponse
	if err := json.NewDecoder(resp.Body).Decode(&syncResp); err != nil {
		return fmt.Errorf("failed to decode initial sync response: %w", err)
	}

	c.syncMu.Lock()
	c.sinceToken = syncResp.NextBatch
	c.syncMu.Unlock()

	logger.DebugCF("matrix", "Initial sync completed", map[string]interface{}{
		"since_token": syncResp.NextBatch,
	})

	return nil
}

// syncLoop runs the long-polling sync loop.
func (c *MatrixChannel) syncLoop() {
	defer c.wg.Done()

	for {
		select {
		case <-c.ctx.Done():
			return
		default:
		}

		if err := c.doSync(); err != nil {
			if c.ctx.Err() != nil {
				return
			}

			logger.ErrorCF("matrix", "Sync error", map[string]interface{}{
				"error": err.Error(),
			})

			// Back off before retrying
			select {
			case <-c.ctx.Done():
				return
			case <-time.After(5 * time.Second):
				continue
			}
		}
	}
}

// doSync performs a single long-poll sync request and processes events.
func (c *MatrixChannel) doSync() error {
	c.syncMu.Lock()
	since := c.sinceToken
	c.syncMu.Unlock()

	// Build URL with timeout for long polling (30 seconds)
	url := fmt.Sprintf("%s/_matrix/client/v3/sync?timeout=30000", c.config.Homeserver)
	if since != "" {
		url += "&since=" + since
	}

	req, err := http.NewRequestWithContext(c.ctx, http.MethodGet, url, nil)
	if err != nil {
		return err
	}
	req.Header.Set("Authorization", "Bearer "+c.config.AccessToken)

	resp, err := c.httpClient.Do(req)
	if err != nil {
		return fmt.Errorf("sync request failed: %w", err)
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		body, _ := io.ReadAll(resp.Body)
		return fmt.Errorf("sync returned status %d: %s", resp.StatusCode, string(body))
	}

	var syncResp matrixSyncResponse
	if err := json.NewDecoder(resp.Body).Decode(&syncResp); err != nil {
		return fmt.Errorf("failed to decode sync response: %w", err)
	}

	// Update the since token
	c.syncMu.Lock()
	c.sinceToken = syncResp.NextBatch
	c.syncMu.Unlock()

	// Process events from joined rooms
	for roomID, roomData := range syncResp.Rooms.Join {
		for _, event := range roomData.Timeline.Events {
			c.processEvent(roomID, event)
		}
	}

	return nil
}

// processEvent handles a single Matrix event.
func (c *MatrixChannel) processEvent(roomID string, event matrixEvent) {
	// Only handle text messages
	if event.Type != "m.room.message" {
		return
	}

	// Ignore our own messages
	if event.Sender == c.config.UserID {
		return
	}

	// Only handle text message types
	if event.Content.MsgType != "m.text" {
		logger.DebugCF("matrix", "Ignoring non-text message", map[string]interface{}{
			"room_id":  roomID,
			"msg_type": event.Content.MsgType,
			"sender":   event.Sender,
		})
		return
	}

	content := event.Content.Body
	if content == "" {
		return
	}

	senderID := event.Sender
	chatID := roomID

	logger.DebugCF("matrix", "Received message", map[string]interface{}{
		"room_id":  roomID,
		"sender":   senderID,
		"event_id": event.EventID,
		"preview":  utils.Truncate(content, 50),
	})

	metadata := map[string]string{
		"platform":  "matrix",
		"event_id":  event.EventID,
		"room_id":   roomID,
		"sender":    senderID,
		"msg_type":  event.Content.MsgType,
	}

	// If a default room is configured, use roomID as the chatID.
	// Otherwise, use roomID directly.
	if c.config.RoomID != "" && roomID != c.config.RoomID {
		// For messages from non-default rooms, include the room in chatID
		chatID = roomID
	}

	c.HandleMessage(senderID, chatID, content, nil, metadata)
}

// sendMessage sends a text message to a Matrix room.
func (c *MatrixChannel) sendMessage(ctx context.Context, roomID, content string) (string, error) {
	// Generate a unique transaction ID based on timestamp
	txnID := fmt.Sprintf("nb_%d", time.Now().UnixNano())

	url := fmt.Sprintf("%s/_matrix/client/v3/rooms/%s/send/m.room.message/%s",
		c.config.Homeserver, roomID, txnID)

	payload := map[string]string{
		"msgtype": "m.text",
		"body":    content,
	}

	body, err := json.Marshal(payload)
	if err != nil {
		return "", fmt.Errorf("failed to marshal message payload: %w", err)
	}

	req, err := http.NewRequestWithContext(ctx, http.MethodPut, url, bytes.NewReader(body))
	if err != nil {
		return "", fmt.Errorf("failed to create request: %w", err)
	}

	req.Header.Set("Authorization", "Bearer "+c.config.AccessToken)
	req.Header.Set("Content-Type", "application/json")

	resp, err := c.httpClient.Do(req)
	if err != nil {
		return "", fmt.Errorf("send request failed: %w", err)
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		respBody, _ := io.ReadAll(resp.Body)
		var merr matrixError
		if json.Unmarshal(respBody, &merr) == nil && merr.ErrCode != "" {
			return "", fmt.Errorf("Matrix error %s: %s", merr.ErrCode, merr.Error)
		}
		return "", fmt.Errorf("send returned status %d: %s", resp.StatusCode, string(respBody))
	}

	var sendResp matrixSendResponse
	if err := json.NewDecoder(resp.Body).Decode(&sendResp); err != nil {
		return "", fmt.Errorf("failed to decode send response: %w", err)
	}

	return sendResp.EventID, nil
}

// Ensure MatrixChannel implements the Channel interface at compile time.
var _ Channel = (*MatrixChannel)(nil)
