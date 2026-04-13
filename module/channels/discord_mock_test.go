// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package channels

import (
	"context"
	"errors"
	"fmt"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/276793422/NemesisBot/module/bus"
	"github.com/276793422/NemesisBot/module/config"
	"github.com/bwmarrin/discordgo"
)

// mockDiscordAPI implements discordAPI for testing
type mockDiscordAPI struct {
	mu          sync.Mutex
	sendCalls   []sendCall
	typingCalls []string
	openErr     error
	closeErr    error
	userResult  *discordgo.User
	userErr     error
	handlers    []interface{}
}

type sendCall struct {
	channelID string
	content   string
}

func (m *mockDiscordAPI) ChannelMessageSend(channelID, content string, options ...discordgo.RequestOption) (*discordgo.Message, error) {
	m.mu.Lock()
	defer m.mu.Unlock()
	m.sendCalls = append(m.sendCalls, sendCall{channelID, content})
	return &discordgo.Message{ID: "msg_" + fmt.Sprintf("%d", len(m.sendCalls))}, nil
}

func (m *mockDiscordAPI) ChannelTyping(channelID string, options ...discordgo.RequestOption) error {
	m.mu.Lock()
	defer m.mu.Unlock()
	m.typingCalls = append(m.typingCalls, channelID)
	return nil
}

func (m *mockDiscordAPI) Open() error {
	return m.openErr
}

func (m *mockDiscordAPI) Close() error {
	return m.closeErr
}

func (m *mockDiscordAPI) User(userID string, options ...discordgo.RequestOption) (*discordgo.User, error) {
	if m.userErr != nil {
		return nil, m.userErr
	}
	if m.userResult != nil {
		return m.userResult, nil
	}
	return &discordgo.User{ID: "bot123", Username: "TestBot"}, nil
}

func (m *mockDiscordAPI) AddHandler(handler interface{}) func() {
	m.handlers = append(m.handlers, handler)
	return func() {}
}

// mockDiscordAPIErrors returns errors on send
type mockDiscordAPIErrors struct {
	sendErr error
}

func (m *mockDiscordAPIErrors) ChannelMessageSend(channelID, content string, options ...discordgo.RequestOption) (*discordgo.Message, error) {
	return nil, m.sendErr
}

func (m *mockDiscordAPIErrors) ChannelTyping(channelID string, options ...discordgo.RequestOption) error { return nil }
func (m *mockDiscordAPIErrors) Open() error                          { return nil }
func (m *mockDiscordAPIErrors) Close() error                         { return nil }
func (m *mockDiscordAPIErrors) User(userID string, options ...discordgo.RequestOption) (*discordgo.User, error) {
	return &discordgo.User{ID: "bot123", Username: "TestBot"}, nil
}
func (m *mockDiscordAPIErrors) AddHandler(handler interface{}) func() { return func() {} }

func newTestDiscordChannel(api discordAPI) *DiscordChannel {
	cfg := config.DiscordConfig{
		Token:     "test-token",
		AllowFrom: []string{},
	}
	msgBus := bus.NewMessageBus()
	return NewDiscordChannelWithClient(cfg, msgBus, api)
}

func newDiscordSession(botUserID string) *discordgo.Session {
	s := &discordgo.Session{}
	s.State = discordgo.NewState()
	s.State.User = &discordgo.User{ID: botUserID}
	return s
}

func TestDiscordChannel_Send_Success(t *testing.T) {
	api := &mockDiscordAPI{}
	ch := newTestDiscordChannel(api)
	ch.setRunning(true)

	err := ch.Send(context.Background(), bus.OutboundMessage{
		Channel: "discord",
		ChatID:  "ch1",
		Content: "hello",
	})
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	api.mu.Lock()
	defer api.mu.Unlock()
	if len(api.sendCalls) != 1 {
		t.Fatalf("expected 1 send call, got %d", len(api.sendCalls))
	}
	if api.sendCalls[0].content != "hello" {
		t.Errorf("content = %q", api.sendCalls[0].content)
	}
}

func TestDiscordChannel_Send_Chunking(t *testing.T) {
	api := &mockDiscordAPI{}
	ch := newTestDiscordChannel(api)
	ch.setRunning(true)

	longMsg := strings.Repeat("A", 2500)
	err := ch.Send(context.Background(), bus.OutboundMessage{
		ChatID:  "ch1",
		Content: longMsg,
	})
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	api.mu.Lock()
	defer api.mu.Unlock()
	if len(api.sendCalls) < 2 {
		t.Errorf("expected at least 2 chunks, got %d", len(api.sendCalls))
	}
}

func TestDiscordChannel_Send_NotRunning(t *testing.T) {
	api := &mockDiscordAPI{}
	ch := newTestDiscordChannel(api)
	// not running

	err := ch.Send(context.Background(), bus.OutboundMessage{
		ChatID:  "ch1",
		Content: "hello",
	})
	if err == nil {
		t.Error("expected error when not running")
	}
}

func TestDiscordChannel_Send_EmptyContent(t *testing.T) {
	api := &mockDiscordAPI{}
	ch := newTestDiscordChannel(api)
	ch.setRunning(true)

	err := ch.Send(context.Background(), bus.OutboundMessage{
		ChatID:  "ch1",
		Content: "",
	})
	if err != nil {
		t.Errorf("unexpected error: %v", err)
	}

	api.mu.Lock()
	defer api.mu.Unlock()
	if len(api.sendCalls) != 0 {
		t.Error("empty content should not trigger send")
	}
}

func TestDiscordChannel_Send_EmptyChatID(t *testing.T) {
	api := &mockDiscordAPI{}
	ch := newTestDiscordChannel(api)
	ch.setRunning(true)

	err := ch.Send(context.Background(), bus.OutboundMessage{
		ChatID:  "",
		Content: "hello",
	})
	if err == nil {
		t.Error("expected error for empty chatID")
	}
}

func TestDiscordChannel_Start_Success(t *testing.T) {
	api := &mockDiscordAPI{
		userResult: &discordgo.User{ID: "bot123", Username: "TestBot"},
	}
	ch := newTestDiscordChannel(api)

	err := ch.Start(context.Background())
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if !ch.IsRunning() {
		t.Error("channel should be running")
	}
}

func TestDiscordChannel_Start_OpenError(t *testing.T) {
	api := &mockDiscordAPI{
		openErr: errors.New("connection refused"),
	}
	ch := newTestDiscordChannel(api)

	err := ch.Start(context.Background())
	if err == nil {
		t.Error("expected error when Open fails")
	}
}

func TestDiscordChannel_Start_UserError(t *testing.T) {
	api := &mockDiscordAPI{
		userErr: errors.New("user not found"),
	}
	ch := newTestDiscordChannel(api)

	err := ch.Start(context.Background())
	if err == nil {
		t.Error("expected error when User fails")
	}
}

func TestDiscordChannel_Stop_Success(t *testing.T) {
	api := &mockDiscordAPI{}
	ch := newTestDiscordChannel(api)
	ch.setRunning(true)

	err := ch.Stop(context.Background())
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if ch.IsRunning() {
		t.Error("channel should not be running")
	}
}

func TestDiscordChannel_Stop_TypingCleanup(t *testing.T) {
	api := &mockDiscordAPI{}
	ch := newTestDiscordChannel(api)
	ch.setRunning(true)

	// Simulate active typing
	ch.typingMu.Lock()
	ch.typingStop["ch1"] = make(chan struct{})
	ch.typingMu.Unlock()

	err := ch.Stop(context.Background())
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	ch.typingMu.Lock()
	defer ch.typingMu.Unlock()
	if len(ch.typingStop) != 0 {
		t.Error("typingStop should be cleaned up")
	}
}

func TestDiscordChannel_Stop_CloseError(t *testing.T) {
	api := &mockDiscordAPI{
		closeErr: errors.New("close error"),
	}
	ch := newTestDiscordChannel(api)
	ch.setRunning(true)

	err := ch.Stop(context.Background())
	if err == nil {
		t.Error("expected close error")
	}
}

func TestDiscordChannel_Typing_StartStop(t *testing.T) {
	api := &mockDiscordAPI{}
	ch := newTestDiscordChannel(api)
	ch.ctx = context.Background()

	// Start typing
	ch.startTyping("ch1")
	time.Sleep(50 * time.Millisecond)

	api.mu.Lock()
	typingCount := len(api.typingCalls)
	api.mu.Unlock()
	if typingCount == 0 {
		t.Error("expected at least one typing call")
	}

	// Stop typing
	ch.stopTyping("ch1")
	time.Sleep(50 * time.Millisecond)

	// Should not panic
	ch.typingMu.Lock()
	_, exists := ch.typingStop["ch1"]
	ch.typingMu.Unlock()
	if exists {
		t.Error("typingStop entry should be removed")
	}
}

func TestDiscordChannel_HandleMessage_NilMessage(t *testing.T) {
	api := &mockDiscordAPI{}
	ch := newTestDiscordChannel(api)

	session := newDiscordSession("bot123")
	// Passing nil MessageCreate — should not panic (nil check in handleMessage)
	ch.handleMessage(session, nil)
}

func TestDiscordChannel_HandleMessage_NilAuthor(t *testing.T) {
	api := &mockDiscordAPI{}
	ch := newTestDiscordChannel(api)

	session := newDiscordSession("bot123")
	msg := &discordgo.MessageCreate{Message: &discordgo.Message{Author: nil}}
	ch.handleMessage(session, msg)
}

func TestDiscordChannel_HandleMessage_Self(t *testing.T) {
	api := &mockDiscordAPI{}
	msgBus := bus.NewMessageBus()

	cfg := config.DiscordConfig{Token: "test", AllowFrom: []string{}}
	ch := NewDiscordChannelWithClient(cfg, msgBus, api)

	session := newDiscordSession("bot123")
	msg := &discordgo.MessageCreate{Message: &discordgo.Message{
		Author:    &discordgo.User{ID: "bot123"},
		Content:   "self message",
		ChannelID: "ch1",
	}}
	// Should return early (self message)
	ch.handleMessage(session, msg)
}

func TestDiscordChannel_HandleMessage_Blocked(t *testing.T) {
	api := &mockDiscordAPI{}
	msgBus := bus.NewMessageBus()
	cfg := config.DiscordConfig{Token: "test", AllowFrom: []string{"allowed_user"}}
	ch := NewDiscordChannelWithClient(cfg, msgBus, api)

	session := newDiscordSession("bot123")
	msg := &discordgo.MessageCreate{Message: &discordgo.Message{
		Author:    &discordgo.User{ID: "blocked_user", Username: "Blocked"},
		Content:   "blocked message",
		ChannelID: "ch1",
	}}
	// Should return early (not allowed)
	ch.handleMessage(session, msg)
}

func TestDiscordChannel_HandleMessage_Allowed(t *testing.T) {
	api := &mockDiscordAPI{}
	msgBus := bus.NewMessageBus()
	cfg := config.DiscordConfig{Token: "test", AllowFrom: []string{"user123"}}
	ch := NewDiscordChannelWithClient(cfg, msgBus, api)
	ch.ctx = context.Background()

	// Subscribe to inbound messages
	received := make(chan bus.InboundMessage, 1)
	go func() {
		ctx, cancel := context.WithTimeout(context.Background(), 500*time.Millisecond)
		defer cancel()
		if msg, ok := msgBus.ConsumeInbound(ctx); ok {
			received <- msg
		}
	}()

	session := newDiscordSession("bot123")
	msg := &discordgo.MessageCreate{Message: &discordgo.Message{
		Author:    &discordgo.User{ID: "user123", Username: "TestUser"},
		Content:   "hello discord",
		ChannelID: "ch1",
		GuildID:   "guild1",
	}}
	ch.handleMessage(session, msg)

	select {
	case inbound := <-received:
		if inbound.Content != "hello discord" {
			t.Errorf("content = %q", inbound.Content)
		}
		if inbound.Channel != "discord" {
			t.Errorf("channel = %q", inbound.Channel)
		}
	case <-time.After(1 * time.Second):
		t.Error("timed out waiting for inbound message")
	}
}

func TestDiscordChannel_HandleMessage_DM(t *testing.T) {
	api := &mockDiscordAPI{}
	msgBus := bus.NewMessageBus()
	cfg := config.DiscordConfig{Token: "test", AllowFrom: []string{"user123"}}
	ch := NewDiscordChannelWithClient(cfg, msgBus, api)
	ch.ctx = context.Background()

	received := make(chan bus.InboundMessage, 1)
	go func() {
		ctx, cancel := context.WithTimeout(context.Background(), 500*time.Millisecond)
		defer cancel()
		if msg, ok := msgBus.ConsumeInbound(ctx); ok {
			received <- msg
		}
	}()

	session := newDiscordSession("bot123")
	msg := &discordgo.MessageCreate{Message: &discordgo.Message{
		Author:    &discordgo.User{ID: "user123", Username: "DMUser"},
		Content:   "dm message",
		ChannelID: "dm_ch1",
		GuildID:   "", // DM = no guild
	}}
	ch.handleMessage(session, msg)

	select {
	case inbound := <-received:
		if inbound.Metadata["is_dm"] != "true" {
			t.Errorf("expected DM message, is_dm = %q", inbound.Metadata["is_dm"])
		}
		if inbound.Metadata["peer_kind"] != "direct" {
			t.Errorf("expected peer_kind=direct, got %q", inbound.Metadata["peer_kind"])
		}
	case <-time.After(1 * time.Second):
		t.Error("timed out")
	}
}

func TestDiscordChannel_Send_SendError(t *testing.T) {
	api := &mockDiscordAPIErrors{sendErr: errors.New("network error")}
	ch := newTestDiscordChannel(api)
	ch.setRunning(true)

	err := ch.Send(context.Background(), bus.OutboundMessage{
		ChatID:  "ch1",
		Content: "hello",
	})
	if err == nil {
		t.Error("expected error from send failure")
	}
}

func TestDiscordChannel_GetContext(t *testing.T) {
	api := &mockDiscordAPI{}
	ch := newTestDiscordChannel(api)

	ctx := ch.getContext()
	if ctx == nil {
		t.Error("context should not be nil")
	}
}
