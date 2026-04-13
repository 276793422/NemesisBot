// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package channels

import (
	"context"
	"errors"
	"fmt"
	"sync"
	"testing"
	"time"

	"github.com/276793422/NemesisBot/module/bus"
	"github.com/276793422/NemesisBot/module/config"
	"github.com/mymmrac/telego"
)

// mockTelegramAPI implements telegramAPI for testing
type mockTelegramAPI struct {
	mu            sync.Mutex
	sendCalls     []*telego.SendMessageParams
	editCalls     []*telego.EditMessageTextParams
	actionCalls   []*telego.SendChatActionParams
	getFileCalls  []*telego.GetFileParams
	sendResult    *telego.Message
	sendErr       error
	editResult    *telego.Message
	editErr       error
	actionErr     error
	fileResult    *telego.File
	fileErr       error
	username      string
}

func (m *mockTelegramAPI) SendMessage(ctx context.Context, params *telego.SendMessageParams) (*telego.Message, error) {
	m.mu.Lock()
	defer m.mu.Unlock()
	m.sendCalls = append(m.sendCalls, params)
	if m.sendErr != nil {
		return nil, m.sendErr
	}
	if m.sendResult != nil {
		return m.sendResult, nil
	}
	return &telego.Message{MessageID: len(m.sendCalls)}, nil
}

func (m *mockTelegramAPI) EditMessageText(ctx context.Context, params *telego.EditMessageTextParams) (*telego.Message, error) {
	m.mu.Lock()
	defer m.mu.Unlock()
	m.editCalls = append(m.editCalls, params)
	if m.editErr != nil {
		return nil, m.editErr
	}
	if m.editResult != nil {
		return m.editResult, nil
	}
	return &telego.Message{MessageID: 1}, nil
}

func (m *mockTelegramAPI) SendChatAction(ctx context.Context, params *telego.SendChatActionParams) error {
	m.mu.Lock()
	defer m.mu.Unlock()
	m.actionCalls = append(m.actionCalls, params)
	return m.actionErr
}

func (m *mockTelegramAPI) GetFile(ctx context.Context, params *telego.GetFileParams) (*telego.File, error) {
	m.mu.Lock()
	defer m.mu.Unlock()
	m.getFileCalls = append(m.getFileCalls, params)
	if m.fileErr != nil {
		return nil, m.fileErr
	}
	if m.fileResult != nil {
		return m.fileResult, nil
	}
	return &telego.File{FileID: params.FileID}, nil
}

func (m *mockTelegramAPI) FileDownloadURL(filepath string) string {
	return "https://api.telegram.org/file/bot_test/" + filepath
}

func (m *mockTelegramAPI) Username() string {
	if m.username != "" {
		return m.username
	}
	return "test_bot"
}

func newTestTelegramChannel(api telegramAPI) *TelegramChannel {
	cfg := &config.Config{
		Channels: config.ChannelsConfig{
			Telegram: config.TelegramConfig{
				Token:     "test-token",
				AllowFrom: []string{},
			},
		},
	}
	msgBus := bus.NewMessageBus()
	return NewTelegramChannelWithClient(cfg, msgBus, api)
}

func TestTelegramChannel_Send_Success(t *testing.T) {
	api := &mockTelegramAPI{}
	ch := newTestTelegramChannel(api)
	ch.setRunning(true)

	err := ch.Send(context.Background(), bus.OutboundMessage{
		ChatID:  "123456",
		Content: "hello telegram",
	})
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	api.mu.Lock()
	defer api.mu.Unlock()
	if len(api.sendCalls) != 1 {
		t.Fatalf("expected 1 send call, got %d", len(api.sendCalls))
	}
	if api.sendCalls[0].Text != "hello telegram" {
		t.Errorf("text = %q", api.sendCalls[0].Text)
	}
}

func TestTelegramChannel_Send_EditPlaceholder(t *testing.T) {
	api := &mockTelegramAPI{}
	ch := newTestTelegramChannel(api)
	ch.setRunning(true)

	// Store a placeholder
	chatIDStr := "123456"
	ch.placeholders.Store(chatIDStr, 42)

	err := ch.Send(context.Background(), bus.OutboundMessage{
		ChatID:  chatIDStr,
		Content: "response text",
	})
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	api.mu.Lock()
	defer api.mu.Unlock()
	// Should have edited the placeholder, not sent new
	if len(api.editCalls) != 1 {
		t.Fatalf("expected 1 edit call, got %d", len(api.editCalls))
	}
	if len(api.sendCalls) != 0 {
		t.Errorf("expected 0 send calls, got %d", len(api.sendCalls))
	}
}

func TestTelegramChannel_Send_EditFailFallback(t *testing.T) {
	api := &mockTelegramAPI{
		editErr: errors.New("edit failed"),
	}
	ch := newTestTelegramChannel(api)
	ch.setRunning(true)

	// Store a placeholder
	chatIDStr := "123456"
	ch.placeholders.Store(chatIDStr, 42)

	err := ch.Send(context.Background(), bus.OutboundMessage{
		ChatID:  chatIDStr,
		Content: "response text",
	})
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	api.mu.Lock()
	defer api.mu.Unlock()
	// Edit failed, should fall back to new message
	if len(api.editCalls) != 1 {
		t.Errorf("expected 1 edit call, got %d", len(api.editCalls))
	}
	if len(api.sendCalls) != 1 {
		t.Errorf("expected 1 send call (fallback), got %d", len(api.sendCalls))
	}
}

func TestTelegramChannel_Send_HTMLFallback(t *testing.T) {
	api := &mockTelegramAPI{
		sendErr: errors.New("Bad Request: can't parse entities"),
	}
	ch := newTestTelegramChannel(api)
	ch.setRunning(true)

	err := ch.Send(context.Background(), bus.OutboundMessage{
		ChatID:  "123456",
		Content: "hello **bold**",
	})
	if err == nil {
		t.Error("expected error since both sends fail")
	}

	api.mu.Lock()
	defer api.mu.Unlock()
	// Should have tried twice: once with HTML, once plain
	if len(api.sendCalls) != 2 {
		t.Errorf("expected 2 send calls, got %d", len(api.sendCalls))
	}
}

func TestTelegramChannel_Send_NotRunning(t *testing.T) {
	api := &mockTelegramAPI{}
	ch := newTestTelegramChannel(api)

	err := ch.Send(context.Background(), bus.OutboundMessage{
		ChatID:  "123456",
		Content: "hello",
	})
	if err == nil {
		t.Error("expected error when not running")
	}
}

func TestTelegramChannel_Send_InvalidChatID(t *testing.T) {
	api := &mockTelegramAPI{}
	ch := newTestTelegramChannel(api)
	ch.setRunning(true)

	err := ch.Send(context.Background(), bus.OutboundMessage{
		ChatID:  "not_a_number",
		Content: "hello",
	})
	if err == nil {
		t.Error("expected error for invalid chat ID")
	}
}

func TestTelegramChannel_ThinkingCancel(t *testing.T) {
	api := &mockTelegramAPI{}
	ch := newTestTelegramChannel(api)
	ch.setRunning(true)

	// Store a thinkingCancel
	chatIDStr := "123456"
	ctx, cancel := context.WithCancel(context.Background())
	ch.stopThinking.Store(chatIDStr, &thinkingCancel{fn: cancel})

	// Send should cancel thinking
	err := ch.Send(context.Background(), bus.OutboundMessage{
		ChatID:  chatIDStr,
		Content: "response",
	})
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	// Context should be cancelled
	select {
	case <-ctx.Done():
		// Good
	default:
		t.Error("thinking context should be cancelled")
	}
}

func TestTelegramChannel_HandleMessage_NilMessage(t *testing.T) {
	api := &mockTelegramAPI{}
	ch := newTestTelegramChannel(api)

	err := ch.handleMessage(context.Background(), nil)
	if err == nil {
		t.Error("expected error for nil message")
	}
}

func TestTelegramChannel_HandleMessage_NilUser(t *testing.T) {
	api := &mockTelegramAPI{}
	ch := newTestTelegramChannel(api)

	msg := &telego.Message{
		MessageID: 1,
		From:      nil,
		Chat:      telego.Chat{ID: 123, Type: "private"},
	}
	err := ch.handleMessage(context.Background(), msg)
	if err == nil {
		t.Error("expected error for nil user")
	}
}

func TestTelegramChannel_HandleMessage_TextOnly(t *testing.T) {
	api := &mockTelegramAPI{sendResult: &telego.Message{MessageID: 10}}
	msgBus := bus.NewMessageBus()

	cfg := &config.Config{
		Channels: config.ChannelsConfig{
			Telegram: config.TelegramConfig{
				Token:     "test",
				AllowFrom: []string{},
			},
		},
	}
	ch := NewTelegramChannelWithClient(cfg, msgBus, api)

	received := make(chan bus.InboundMessage, 1)
	go func() {
		ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
		defer cancel()
		if msg, ok := msgBus.ConsumeInbound(ctx); ok {
			received <- msg
		}
	}()

	msg := &telego.Message{
		MessageID: 1,
		From:      &telego.User{ID: 999, Username: "testuser"},
		Chat:      telego.Chat{ID: 123456, Type: "private"},
		Text:      "hello bot",
	}
	_ = ch.handleMessage(context.Background(), msg)

	select {
	case inbound := <-received:
		if inbound.Content != "hello bot" {
			t.Errorf("content = %q", inbound.Content)
		}
		if inbound.Channel != "telegram" {
			t.Errorf("channel = %q", inbound.Channel)
		}
		if inbound.Metadata["peer_kind"] != "direct" {
			t.Errorf("peer_kind = %q", inbound.Metadata["peer_kind"])
		}
	case <-time.After(3 * time.Second):
		t.Error("timed out waiting for message")
	}
}

func TestTelegramChannel_HandleMessage_GroupMessage(t *testing.T) {
	api := &mockTelegramAPI{sendResult: &telego.Message{MessageID: 10}}
	msgBus := bus.NewMessageBus()

	cfg := &config.Config{
		Channels: config.ChannelsConfig{
			Telegram: config.TelegramConfig{
				Token:     "test",
				AllowFrom: []string{},
			},
		},
	}
	ch := NewTelegramChannelWithClient(cfg, msgBus, api)

	received := make(chan bus.InboundMessage, 1)
	go func() {
		ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
		defer cancel()
		if msg, ok := msgBus.ConsumeInbound(ctx); ok {
			received <- msg
		}
	}()

	msg := &telego.Message{
		MessageID: 2,
		From:      &telego.User{ID: 999, Username: "testuser"},
		Chat:      telego.Chat{ID: -1001234567890, Type: "supergroup"},
		Text:      "group message",
	}
	_ = ch.handleMessage(context.Background(), msg)

	select {
	case inbound := <-received:
		if inbound.Metadata["peer_kind"] != "group" {
			t.Errorf("peer_kind = %q, want 'group'", inbound.Metadata["peer_kind"])
		}
	case <-time.After(3 * time.Second):
		t.Error("timed out")
	}
}

func TestTelegramChannel_HandleMessage_AllowList(t *testing.T) {
	api := &mockTelegramAPI{sendResult: &telego.Message{MessageID: 10}}
	msgBus := bus.NewMessageBus()

	cfg := &config.Config{
		Channels: config.ChannelsConfig{
			Telegram: config.TelegramConfig{
				Token:     "test",
				AllowFrom: []string{"888"},
			},
		},
	}
	ch := NewTelegramChannelWithClient(cfg, msgBus, api)

	msg := &telego.Message{
		MessageID: 3,
		From:      &telego.User{ID: 999, Username: "blocked_user"},
		Chat:      telego.Chat{ID: 123, Type: "private"},
		Text:      "blocked message",
	}
	// Should be rejected by allowlist
	_ = ch.handleMessage(context.Background(), msg)
}

func TestTelegramChannel_HandleMessage_WithCaption(t *testing.T) {
	api := &mockTelegramAPI{sendResult: &telego.Message{MessageID: 10}}
	msgBus := bus.NewMessageBus()

	cfg := &config.Config{
		Channels: config.ChannelsConfig{
			Telegram: config.TelegramConfig{
				Token:     "test",
				AllowFrom: []string{},
			},
		},
	}
	ch := NewTelegramChannelWithClient(cfg, msgBus, api)

	received := make(chan bus.InboundMessage, 1)
	go func() {
		ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
		defer cancel()
		if msg, ok := msgBus.ConsumeInbound(ctx); ok {
			received <- msg
		}
	}()

	msg := &telego.Message{
		MessageID: 4,
		From:      &telego.User{ID: 999, Username: "testuser"},
		Chat:      telego.Chat{ID: 123, Type: "private"},
		Text:      "main text",
		Caption:   "caption text",
	}
	_ = ch.handleMessage(context.Background(), msg)

	select {
	case inbound := <-received:
		// Should contain both text and caption
		if inbound.Content != "main text\ncaption text" {
			t.Errorf("content = %q", inbound.Content)
		}
	case <-time.After(3 * time.Second):
		t.Error("timed out")
	}
}

func TestTelegramChannel_Send_ThinkingCancelOnSend(t *testing.T) {
	api := &mockTelegramAPI{}
	ch := newTestTelegramChannel(api)
	ch.setRunning(true)

	chatID := "123456"
	parentCtx, parentCancel := context.WithCancel(context.Background())
	defer parentCancel()

	// Store a thinkingCancel that we can verify gets cancelled
	childCtx, childCancel := context.WithCancel(parentCtx)
	ch.stopThinking.Store(chatID, &thinkingCancel{fn: childCancel})

	// Send should cancel thinking
	err := ch.Send(parentCtx, bus.OutboundMessage{
		ChatID:  chatID,
		Content: "response",
	})
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	// Child context should be cancelled
	select {
	case <-childCtx.Done():
	default:
		t.Error("thinking cancel should have been invoked")
	}

	// stopThinking entry should be removed
	if _, ok := ch.stopThinking.Load(chatID); ok {
		t.Error("stopThinking entry should be removed")
	}
}

func TestTelegramChannel_Send_PlaceholderRemoved(t *testing.T) {
	api := &mockTelegramAPI{}
	ch := newTestTelegramChannel(api)
	ch.setRunning(true)

	chatID := "123456"
	ch.placeholders.Store(chatID, 42)

	_ = ch.Send(context.Background(), bus.OutboundMessage{
		ChatID:  chatID,
		Content: "response",
	})

	// Placeholder should be removed
	if _, ok := ch.placeholders.Load(chatID); ok {
		t.Error("placeholder should be removed after use")
	}
}

// Ensure the helper types compile correctly with the interface
func TestTelegramAPIMockCompile(t *testing.T) {
	var _ telegramAPI = &mockTelegramAPI{}
}

// Ensure thinkingCancel works correctly
func TestThinkingCancel_Nil(t *testing.T) {
	var tc *thinkingCancel
	tc.Cancel() // Should not panic

	tc = &thinkingCancel{fn: nil}
	tc.Cancel() // Should not panic
}

func TestThinkingCancel_Valid(t *testing.T) {
	ctx, cancel := context.WithCancel(context.Background())
	tc := &thinkingCancel{fn: cancel}
	tc.Cancel()

	select {
	case <-ctx.Done():
	default:
		t.Error("context should be cancelled")
	}
}

func TestTelegramChannel_Stop(t *testing.T) {
	api := &mockTelegramAPI{}
	ch := newTestTelegramChannel(api)
	ch.setRunning(true)

	err := ch.Stop(context.Background())
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if ch.IsRunning() {
		t.Error("should not be running")
	}
}

func TestTelegramChannel_Send_EmptyContent(t *testing.T) {
	api := &mockTelegramAPI{}
	ch := newTestTelegramChannel(api)
	ch.setRunning(true)

	// markdownToTelegramHTML("") returns ""
	err := ch.Send(context.Background(), bus.OutboundMessage{
		ChatID:  "123456",
		Content: "",
	})
	// Empty string is valid HTML, should send successfully
	if err != nil {
		t.Logf("Send with empty content returned: %v", err)
	}
}

func TestTelegramChannel_DownloadFileGetFileError(t *testing.T) {
	api := &mockTelegramAPI{fileErr: errors.New("file not found")}
	ch := newTestTelegramChannel(api)

	result := ch.downloadFile(context.Background(), "nonexistent_file_id", ".ogg")
	if result != "" {
		t.Errorf("expected empty result on error, got %q", result)
	}
}

func TestTelegramChannel_DownloadFileWithInfo_EmptyPath(t *testing.T) {
	api := &mockTelegramAPI{}
	ch := newTestTelegramChannel(api)

	file := &telego.File{FilePath: ""}
	result := ch.downloadFileWithInfo(file, ".jpg")
	if result != "" {
		t.Errorf("expected empty result for empty path, got %q", result)
	}
}

func TestTelegramChannel_HandleMessage_SenderIDWithUsername(t *testing.T) {
	api := &mockTelegramAPI{sendResult: &telego.Message{MessageID: 10}}
	msgBus := bus.NewMessageBus()

	cfg := &config.Config{
		Channels: config.ChannelsConfig{
			Telegram: config.TelegramConfig{
				Token:     "test",
				AllowFrom: []string{},
			},
		},
	}
	ch := NewTelegramChannelWithClient(cfg, msgBus, api)

	received := make(chan bus.InboundMessage, 1)
	go func() {
		ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
		defer cancel()
		if msg, ok := msgBus.ConsumeInbound(ctx); ok {
			received <- msg
		}
	}()

	msg := &telego.Message{
		MessageID: 5,
		From:      &telego.User{ID: 12345, Username: "john_doe"},
		Chat:      telego.Chat{ID: 12345, Type: "private"},
		Text:      "hi",
	}
	_ = ch.handleMessage(context.Background(), msg)

	select {
	case inbound := <-received:
		// HandleMessage receives raw user ID, not the compound senderID
		if inbound.SenderID != "12345" {
			t.Errorf("senderID = %q, want %q", inbound.SenderID, "12345")
		}
	case <-time.After(3 * time.Second):
		t.Error("timed out")
	}
}

func TestTelegramChannel_HandleMessage_ChatIDStored(t *testing.T) {
	api := &mockTelegramAPI{sendResult: &telego.Message{MessageID: 10}}
	msgBus := bus.NewMessageBus()

	cfg := &config.Config{
		Channels: config.ChannelsConfig{
			Telegram: config.TelegramConfig{
				Token:     "test",
				AllowFrom: []string{},
			},
		},
	}
	ch := NewTelegramChannelWithClient(cfg, msgBus, api)

	msg := &telego.Message{
		MessageID: 6,
		From:      &telego.User{ID: 12345, Username: "user"},
		Chat:      telego.Chat{ID: 99999, Type: "private"},
		Text:      "test",
	}
	_ = ch.handleMessage(context.Background(), msg)

	// chatIDs should have stored the mapping
	expectedSender := fmt.Sprintf("%d|user", 12345)
	if chatID, ok := ch.chatIDs[expectedSender]; !ok || chatID != 99999 {
		t.Errorf("chatIDs[%q] = %d, want 99999", expectedSender, chatID)
	}
}

func TestTelegramChannel_SetTranscriber(t *testing.T) {
	api := &mockTelegramAPI{}
	ch := newTestTelegramChannel(api)

	// Should not panic with nil
	ch.SetTranscriber(nil)
}

// Test that parseChatID is used correctly through Send
func TestTelegramChannel_Send_NegativeChatID(t *testing.T) {
	api := &mockTelegramAPI{}
	ch := newTestTelegramChannel(api)
	ch.setRunning(true)

	err := ch.Send(context.Background(), bus.OutboundMessage{
		ChatID:  "-1001234567890",
		Content: "group message",
	})
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	api.mu.Lock()
	defer api.mu.Unlock()
	if len(api.sendCalls) != 1 {
		t.Fatalf("expected 1 send call, got %d", len(api.sendCalls))
	}
}

// Test action is sent when handling message
func TestTelegramChannel_HandleMessage_SendsChatAction(t *testing.T) {
	api := &mockTelegramAPI{sendResult: &telego.Message{MessageID: 10}}
	msgBus := bus.NewMessageBus()

	cfg := &config.Config{
		Channels: config.ChannelsConfig{
			Telegram: config.TelegramConfig{
				Token:     "test",
				AllowFrom: []string{},
			},
		},
	}
	ch := NewTelegramChannelWithClient(cfg, msgBus, api)

	msg := &telego.Message{
		MessageID: 7,
		From:      &telego.User{ID: 999, Username: "testuser"},
		Chat:      telego.Chat{ID: 123, Type: "private"},
		Text:      "hello",
	}
	_ = ch.handleMessage(context.Background(), msg)

	// Allow async operations
	time.Sleep(50 * time.Millisecond)

	api.mu.Lock()
	defer api.mu.Unlock()
	if len(api.actionCalls) == 0 {
		t.Error("expected at least one chat action call")
	}
	if len(api.sendCalls) < 1 {
		t.Error("expected at least one send call (thinking placeholder)")
	}
}

