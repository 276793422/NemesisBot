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
	"github.com/276793422/NemesisBot/module/utils"
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

// pendingRequest 表示来自 RPC 的待处理 LLM 请求
type pendingRequest struct {
	correlationID string
	responseCh    chan string
	createdAt     time.Time
	timeout       time.Duration
	delivered     bool // delivered 标记响应是否已成功发送给 handler
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

// Send implements Channel interface - receives messages from dispatchOutbound
// and delivers them to waiting RPC handlers
func (ch *RPCChannel) Send(ctx context.Context, msg bus.OutboundMessage) error {
	logger.InfoCF("rpc", "RPCChannel.Send called",
		map[string]interface{}{
			"msg_channel": msg.Channel,
			"ch_name":     ch.Name(),
			"chat_id":     msg.ChatID,
			"content_len": len(msg.Content),
		})

	// Only process messages from this channel
	if msg.Channel != ch.Name() {
		logger.WarnCF("rpc", "Channel mismatch - message not for this channel",
			map[string]interface{}{
				"msg_channel": msg.Channel,
				"ch_name":     ch.Name(),
			})
		return nil
	}

	logger.InfoCF("rpc", "Channel matched, processing message",
		map[string]interface{}{
			"content_preview": utils.Truncate(msg.Content, 100),
		})

	// Extract correlation ID from content
	// Format: "[rpc:correlation_id] actual response"
	correlationID := extractCorrelationID(msg.Content)
	if correlationID == "" {
		logger.WarnCF("rpc", "No correlation ID in message",
			map[string]interface{}{
				"content": msg.Content,
			})
		return nil
	}

	logger.InfoCF("rpc", "Extracted correlation ID from message",
		map[string]interface{}{
			"correlation_id": correlationID,
		})

	// Find pending request and deliver response
	ch.mu.RLock()
	req, exists := ch.pendingReqs[correlationID]
	ch.mu.RUnlock()

	if exists {
		actualContent := removeCorrelationID(msg.Content)
		logger.InfoCF("rpc", "Found pending request, delivering response",
			map[string]interface{}{
				"correlation_id":  correlationID,
				"content_len":     len(actualContent),
				"content_preview": utils.Truncate(actualContent, 100),
				"respCh_ptr":      fmt.Sprintf("%p", req.responseCh),
			})

		select {
		case req.responseCh <- actualContent:
			logger.InfoCF("rpc", "✅ Response delivered successfully via Send",
				map[string]interface{}{
					"correlation_id": correlationID,
				})

			// 标记为已投递，防止 cleanup 关闭此 channel
			//
			// 重要说明："delivered" 标记和延迟删除的设计原理
			//
			// 背景：
			// - 之前的问题是：Send() 成功投递响应后，不会从 pendingReqs 删除记录
			// - 这导致 pendingReqs 无限制增长（例如：每秒 10 个请求，58 分钟内积累 34,800 条记录）
			// - cleanupExpiredRequests() 最终会在超时后删除这些记录，但存在以下问题：
			//   1. 在超时期间内存使用量不必要地增长
			//   2. cleanup 每 30 秒必须遍历越来越大的 map
			//   3. 竞态条件风险：cleanup 可能关闭 handler 仍在使用的 channel
			//
			// 为什么不在 Send() 成功后立即删除记录？
			// - Channel 是带缓冲的（buffer=1），Send() 写入 buffer 后立即返回
			// - Handler（PeerChatHandler）异步从 buffer 读取
			// - 立即删除通常是安全的，因为数据已在 buffer 中
			// - 但是：如果删除了，cleanupExpiredRequests 无法区分"已投递但 handler 处理慢"和"从未投递"
			//
			// 解决方案："delivered" 标记 + 延迟删除
			// - Send() 成功后设置 delivered=true
			// - cleanupExpiredRequests 检查此标记：
			//   * 如果 !delivered && 超时：请求失败/已放弃，关闭 channel 通知 handler
			//   * 如果 delivered && 超时：响应已成功发送，安全删除无需关闭 channel
			// - 这样可以防止 cleanup 关闭 handler 仍在使用的 channel
			//
			// 如果您遇到以下相关问题：
			// - pendingReqs 内存泄漏
			// - channel 关闭的竞态条件
			// - handler 收到 "closed channel" 错误
			// 请查看此逻辑和 cleanupExpiredRequests() 的实现
			//
			// 相关分析：参见 docs/BUG/2026-03-11_RPC_CHANNEL_MEMORY_LEAK_ANALYSIS.md
			ch.mu.Lock()
			req.delivered = true
			ch.mu.Unlock()

			logger.DebugCF("rpc", "Marked request as delivered", map[string]interface{}{
				"correlation_id": correlationID,
			})

		case <-time.After(time.Second):
			logger.WarnCF("rpc", "Failed to deliver response (channel full or closed)",
				map[string]interface{}{
					"correlation_id": correlationID,
				})
		}
	} else {
		logger.WarnCF("rpc", "⚠️ No pending request found for correlation ID",
			map[string]interface{}{
				"correlation_id": correlationID,
				"pending_count":  len(ch.pendingReqs),
			})

		// List all pending correlation IDs for debugging
		ch.mu.RLock()
		ids := make([]string, 0, len(ch.pendingReqs))
		for id := range ch.pendingReqs {
			ids = append(ids, id)
		}
		ch.mu.RUnlock()

		if len(ids) > 0 {
			logger.DebugCF("rpc", "Pending correlation IDs",
				map[string]interface{}{
					"ids": ids,
				})
		}
	}

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
		delivered:     false, // Initially not delivered
	}
	ch.mu.Unlock()

	logger.InfoCF("rpc", "Created pending request", map[string]interface{}{
		"correlation_id": inbound.CorrelationID,
		"chat_id":        inbound.ChatID,
		"content_len":    len(inbound.Content),
		"respCh_ptr":     fmt.Sprintf("%p", respCh),
	})

	// Send to MessageBus
	ch.base.bus.PublishInbound(*inbound)

	return respCh, nil
}

// outboundListener is deprecated - messages are now received via Send() method
// This method is kept for backward compatibility but does nothing
func (ch *RPCChannel) outboundListener(ctx context.Context) {
	defer ch.wg.Done()

	logger.InfoC("rpc", "Outbound listener started (deprecated - using Send() method)")

	// Simply wait for stop signal
	select {
	case <-ch.stopCh:
		logger.DebugC("rpc", "Outbound listener stopped (signal)")
	case <-ctx.Done():
		logger.DebugC("rpc", "Outbound listener stopped (context)")
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

// cleanupExpiredRequests 清理已超时的待处理请求
// 根据响应是否已投递使用不同的清理策略：
// - 未投递 + 超时：请求失败/已放弃，关闭 channel 通知 handler
// - 已投递 + 超时：响应已成功发送，安全删除无需关闭 channel
func (ch *RPCChannel) cleanupExpiredRequests() {
	ch.mu.Lock()
	defer ch.mu.Unlock()

	now := time.Now()
	expiredCount := 0
	deliveredCount := 0

	for correlationID, req := range ch.pendingReqs {
		age := now.Sub(req.createdAt)

		if !req.delivered && age > req.timeout {
			// 请求未投递且已超时
			// 这表示一个失败/已放弃的请求
			// 关闭 channel 以通知任何正在等待的 handler
			close(req.responseCh)
			delete(ch.pendingReqs, correlationID)
			expiredCount++
			logger.DebugCF("rpc", "Expired undelivered request (closed channel)", map[string]interface{}{
				"correlation_id": correlationID,
				"age_seconds":    age.Seconds(),
			})
		} else if req.delivered && age > req.timeout {
			// 响应已成功投递，但记录仍在 map 中
			// 可以安全删除而不关闭 channel，因为：
			// 1. 数据已经在带缓冲的 channel 中（buffer=1）
			// 2. Handler 会从 buffer 读取
			// 3. 不会有其他人引用此请求
			delete(ch.pendingReqs, correlationID)
			deliveredCount++
			logger.DebugCF("rpc", "Cleaned delivered request (no channel close)", map[string]interface{}{
				"correlation_id": correlationID,
				"age_seconds":    age.Seconds(),
			})
		}
	}

	if expiredCount > 0 {
		logger.DebugCF("rpc", "Cleaned expired undelivered requests", map[string]interface{}{
			"count": expiredCount,
		})
	}
	if deliveredCount > 0 {
		logger.DebugCF("rpc", "Cleaned delivered requests", map[string]interface{}{
			"count": deliveredCount,
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
