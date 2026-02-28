// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package channels

import (
	"context"
	"fmt"
	"sync"

	"github.com/276793422/NemesisBot/module/bus"
	"github.com/276793422/NemesisBot/module/config"
	"github.com/276793422/NemesisBot/module/constants"
	"github.com/276793422/NemesisBot/module/logger"
)

type Manager struct {
	channels     map[string]Channel
	bus          *bus.MessageBus
	config       *config.Config
	dispatchTask *asyncTask
	mu           sync.RWMutex
}

type asyncTask struct {
	cancel context.CancelFunc
}

func NewManager(cfg *config.Config, messageBus *bus.MessageBus) (*Manager, error) {
	m := &Manager{
		channels: make(map[string]Channel),
		bus:      messageBus,
		config:   cfg,
	}

	if err := m.initChannels(); err != nil {
		return nil, err
	}

	return m, nil
}

func (m *Manager) initChannels() error {
	logger.InfoC("channels", "Initializing channel manager")

	if m.config.Channels.Telegram.Enabled && m.config.Channels.Telegram.Token != "" {
		logger.DebugC("channels", "Attempting to initialize Telegram channel")
		telegram, err := NewTelegramChannel(m.config, m.bus)
		if err != nil {
			logger.ErrorCF("channels", "Failed to initialize Telegram channel", map[string]interface{}{
				"error": err.Error(),
			})
		} else {
			m.channels["telegram"] = telegram
			logger.InfoC("channels", "Telegram channel enabled successfully")
		}
	}

	if m.config.Channels.WhatsApp.Enabled && m.config.Channels.WhatsApp.BridgeURL != "" {
		logger.DebugC("channels", "Attempting to initialize WhatsApp channel")
		whatsapp, err := NewWhatsAppChannel(m.config.Channels.WhatsApp, m.bus)
		if err != nil {
			logger.ErrorCF("channels", "Failed to initialize WhatsApp channel", map[string]interface{}{
				"error": err.Error(),
			})
		} else {
			m.channels["whatsapp"] = whatsapp
			logger.InfoC("channels", "WhatsApp channel enabled successfully")
		}
	}

	if m.config.Channels.Feishu.Enabled {
		logger.DebugC("channels", "Attempting to initialize Feishu channel")
		feishu, err := NewFeishuChannel(m.config.Channels.Feishu, m.bus)
		if err != nil {
			logger.ErrorCF("channels", "Failed to initialize Feishu channel", map[string]interface{}{
				"error": err.Error(),
			})
		} else {
			m.channels["feishu"] = feishu
			logger.InfoC("channels", "Feishu channel enabled successfully")
		}
	}

	if m.config.Channels.Discord.Enabled && m.config.Channels.Discord.Token != "" {
		logger.DebugC("channels", "Attempting to initialize Discord channel")
		discord, err := NewDiscordChannel(m.config.Channels.Discord, m.bus)
		if err != nil {
			logger.ErrorCF("channels", "Failed to initialize Discord channel", map[string]interface{}{
				"error": err.Error(),
			})
		} else {
			m.channels["discord"] = discord
			logger.InfoC("channels", "Discord channel enabled successfully")
		}
	}

	if m.config.Channels.MaixCam.Enabled {
		logger.DebugC("channels", "Attempting to initialize MaixCam channel")
		maixcam, err := NewMaixCamChannel(m.config.Channels.MaixCam, m.bus)
		if err != nil {
			logger.ErrorCF("channels", "Failed to initialize MaixCam channel", map[string]interface{}{
				"error": err.Error(),
			})
		} else {
			m.channels["maixcam"] = maixcam
			logger.InfoC("channels", "MaixCam channel enabled successfully")
		}
	}

	if m.config.Channels.QQ.Enabled {
		logger.DebugC("channels", "Attempting to initialize QQ channel")
		qq, err := NewQQChannel(m.config.Channels.QQ, m.bus)
		if err != nil {
			logger.ErrorCF("channels", "Failed to initialize QQ channel", map[string]interface{}{
				"error": err.Error(),
			})
		} else {
			m.channels["qq"] = qq
			logger.InfoC("channels", "QQ channel enabled successfully")
		}
	}

	if m.config.Channels.DingTalk.Enabled && m.config.Channels.DingTalk.ClientID != "" {
		logger.DebugC("channels", "Attempting to initialize DingTalk channel")
		dingtalk, err := NewDingTalkChannel(m.config.Channels.DingTalk, m.bus)
		if err != nil {
			logger.ErrorCF("channels", "Failed to initialize DingTalk channel", map[string]interface{}{
				"error": err.Error(),
			})
		} else {
			m.channels["dingtalk"] = dingtalk
			logger.InfoC("channels", "DingTalk channel enabled successfully")
		}
	}

	if m.config.Channels.Slack.Enabled && m.config.Channels.Slack.BotToken != "" {
		logger.DebugC("channels", "Attempting to initialize Slack channel")
		slackCh, err := NewSlackChannel(m.config.Channels.Slack, m.bus)
		if err != nil {
			logger.ErrorCF("channels", "Failed to initialize Slack channel", map[string]interface{}{
				"error": err.Error(),
			})
		} else {
			m.channels["slack"] = slackCh
			logger.InfoC("channels", "Slack channel enabled successfully")
		}
	}

	if m.config.Channels.LINE.Enabled && m.config.Channels.LINE.ChannelAccessToken != "" {
		logger.DebugC("channels", "Attempting to initialize LINE channel")
		line, err := NewLINEChannel(m.config.Channels.LINE, m.bus)
		if err != nil {
			logger.ErrorCF("channels", "Failed to initialize LINE channel", map[string]interface{}{
				"error": err.Error(),
			})
		} else {
			m.channels["line"] = line
			logger.InfoC("channels", "LINE channel enabled successfully")
		}
	}

	if m.config.Channels.OneBot.Enabled && m.config.Channels.OneBot.WSUrl != "" {
		logger.DebugC("channels", "Attempting to initialize OneBot channel")
		onebot, err := NewOneBotChannel(m.config.Channels.OneBot, m.bus)
		if err != nil {
			logger.ErrorCF("channels", "Failed to initialize OneBot channel", map[string]interface{}{
				"error": err.Error(),
			})
		} else {
			m.channels["onebot"] = onebot
			logger.InfoC("channels", "OneBot channel enabled successfully")
		}
	}

	// Initialize Web Channel (always available, enabled by default)
	if m.config.Channels.Web.Enabled {
		logger.DebugC("channels", "Attempting to initialize Web channel")
		web, err := NewWebChannel(&m.config.Channels.Web, m.bus)
		if err != nil {
			logger.ErrorCF("channels", "Failed to initialize Web channel", map[string]interface{}{
				"error": err.Error(),
			})
		} else {
			m.channels["web"] = web
			logger.InfoC("channels", "Web channel enabled successfully")
		}
	}

	// Initialize External Channel
	if m.config.Channels.External.Enabled {
		logger.DebugC("channels", "Attempting to initialize External channel")
		external, err := NewExternalChannel(&m.config.Channels.External, m.bus)
		if err != nil {
			logger.ErrorCF("channels", "Failed to initialize External channel", map[string]interface{}{
				"error": err.Error(),
			})
		} else {
			m.channels["external"] = external
			logger.InfoC("channels", "External channel enabled successfully")
		}
	}

	// Initialize WebSocket Channel
	if m.config.Channels.WebSocket.Enabled {
		logger.DebugC("channels", "Attempting to initialize WebSocket channel")
		ws, err := NewWebSocketChannel(&m.config.Channels.WebSocket, m.bus)
		if err != nil {
			logger.ErrorCF("channels", "Failed to initialize WebSocket channel", map[string]interface{}{
				"error": err.Error(),
			})
		} else {
			m.channels["websocket"] = ws
			logger.InfoC("channels", "WebSocket channel enabled successfully")
		}
	}

	logger.InfoCF("channels", "Channel initialization completed", map[string]interface{}{
		"enabled_channels": len(m.channels),
	})

	return nil
}

func (m *Manager) StartAll(ctx context.Context) error {
	m.mu.Lock()
	defer m.mu.Unlock()

	if len(m.channels) == 0 {
		logger.WarnC("channels", "No channels enabled")
		return nil
	}

	logger.InfoC("channels", "Starting all channels")

	// Start unified dispatcher
	dispatchCtx, cancel := context.WithCancel(ctx)
	m.dispatchTask = &asyncTask{cancel: cancel}

	go m.dispatchOutbound(dispatchCtx)

	// Setup sync targets for all channels
	m.setupSyncTargets()

	// Start all channels
	for name, channel := range m.channels {
		logger.InfoCF("channels", "Starting channel", map[string]interface{}{
			"channel": name,
		})
		if err := channel.Start(ctx); err != nil {
			logger.ErrorCF("channels", "Failed to start channel", map[string]interface{}{
				"channel": name,
				"error":   err.Error(),
			})
		}
	}

	logger.InfoC("channels", "All channels started")
	return nil
}

func (m *Manager) StopAll(ctx context.Context) error {
	m.mu.Lock()
	defer m.mu.Unlock()

	logger.InfoC("channels", "Stopping all channels")

	if m.dispatchTask != nil {
		m.dispatchTask.cancel()
		m.dispatchTask = nil
	}

	for name, channel := range m.channels {
		logger.InfoCF("channels", "Stopping channel", map[string]interface{}{
			"channel": name,
		})
		if err := channel.Stop(ctx); err != nil {
			logger.ErrorCF("channels", "Error stopping channel", map[string]interface{}{
				"channel": name,
				"error":   err.Error(),
			})
		}
	}

	logger.InfoC("channels", "All channels stopped")
	return nil
}

func (m *Manager) dispatchOutbound(ctx context.Context) {
	for {
		select {
		case <-ctx.Done():
			return
		case msg, ok := <-m.bus.OutboundChannel():
			if !ok {
				return
			}

			// Silently skip internal channels
			if constants.IsInternalChannel(msg.Channel) {
				continue
			}

			m.mu.RLock()
			channel, exists := m.channels[msg.Channel]
			m.mu.RUnlock()

			if !exists {
				logger.ErrorCF("channels", "Unknown channel for outbound message", map[string]interface{}{
					"channel": msg.Channel,
				})
				continue
			}

			if err := channel.Send(ctx, msg); err != nil {
				logger.ErrorCF("channels", "Error sending message to channel", map[string]interface{}{
					"channel": msg.Channel,
					"error":   err.Error(),
				})
			}
		}
	}
}

func (m *Manager) GetChannel(name string) (Channel, bool) {
	m.mu.RLock()
	defer m.mu.RUnlock()
	channel, ok := m.channels[name]
	return channel, ok
}

func (m *Manager) GetStatus() map[string]interface{} {
	m.mu.RLock()
	defer m.mu.RUnlock()

	status := make(map[string]interface{})
	for name, channel := range m.channels {
		status[name] = map[string]interface{}{
			"enabled": true,
			"running": channel.IsRunning(),
		}
	}
	return status
}

func (m *Manager) GetEnabledChannels() []string {
	m.mu.RLock()
	defer m.mu.RUnlock()

	names := make([]string, 0, len(m.channels))
	for name := range m.channels {
		names = append(names, name)
	}
	return names
}

func (m *Manager) RegisterChannel(name string, channel Channel) {
	m.mu.Lock()
	defer m.mu.Unlock()
	m.channels[name] = channel
}

func (m *Manager) UnregisterChannel(name string) {
	m.mu.Lock()
	defer m.mu.Unlock()
	delete(m.channels, name)
}

func (m *Manager) SendToChannel(ctx context.Context, channelName, chatID, content string) error {
	m.mu.RLock()
	channel, exists := m.channels[channelName]
	m.mu.RUnlock()

	if !exists {
		return fmt.Errorf("channel %s not found", channelName)
	}

	msg := bus.OutboundMessage{
		Channel: channelName,
		ChatID:  chatID,
		Content: content,
	}

	return channel.Send(ctx, msg)
}

// setupSyncTargets establishes sync relationships between channels based on config
func (m *Manager) setupSyncTargets() {
	logger.InfoC("channels", "Setting up sync targets")

	for sourceName, sourceCh := range m.channels {
		// Get sync targets from config (generic method)
		syncTargets := m.getSyncTargets(sourceName)

		logger.DebugCF("channels", "Checking channel for sync targets", map[string]interface{}{
			"channel":      sourceName,
			"sync_targets":  syncTargets,
			"num_targets":  len(syncTargets),
		})

		if len(syncTargets) == 0 {
			continue
		}

		// Establish reference for each sync target
		for _, targetName := range syncTargets {
			// Prevent self-sync
			if targetName == sourceName {
				logger.WarnCF("channels", "Channel cannot sync to itself, skipping",
					map[string]interface{}{
						"channel": sourceName,
					})
				continue
			}

			targetCh, exists := m.channels[targetName]
			if !exists {
				logger.WarnCF("channels", "Sync target not found, skipping",
					map[string]interface{}{
						"source": sourceName,
						"target": targetName,
					})
				continue
			}

			// Add sync target
			if err := sourceCh.AddSyncTarget(targetName, targetCh); err != nil {
				logger.WarnCF("channels", "Failed to add sync target",
					map[string]interface{}{
						"source": sourceName,
						"target": targetName,
						"error":  err.Error(),
					})
				continue
			}

			logger.InfoC("channels", fmt.Sprintf("Linked %s → %s for sync", sourceName, targetName))
		}
	}

	logger.InfoC("channels", "Sync targets setup completed")
}

// getSyncTargets returns the list of sync targets for a channel from config
// This is a generic method that works for all channels
func (m *Manager) getSyncTargets(channelName string) []string {
	var targets []string

	switch channelName {
	case "web":
		targets = m.config.Channels.Web.SyncTo
	case "external":
		targets = m.config.Channels.External.SyncTo
	case "websocket":
		targets = m.config.Channels.WebSocket.SyncTo
	case "telegram":
		targets = m.config.Channels.Telegram.SyncTo
	case "discord":
		targets = m.config.Channels.Discord.SyncTo
	case "whatsapp":
		targets = m.config.Channels.WhatsApp.SyncTo
	case "feishu":
		targets = m.config.Channels.Feishu.SyncTo
	case "slack":
		targets = m.config.Channels.Slack.SyncTo
	case "line":
		targets = m.config.Channels.LINE.SyncTo
	case "onebot":
		targets = m.config.Channels.OneBot.SyncTo
	case "qq":
		targets = m.config.Channels.QQ.SyncTo
	case "dingtalk":
		targets = m.config.Channels.DingTalk.SyncTo
	case "maixcam":
		targets = m.config.Channels.MaixCam.SyncTo
	}

	return targets
}
