// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
// Web Channel Module - Session Management

package web

import (
	"context"
	"fmt"
	"strings"
	"sync"
	"time"

	"github.com/google/uuid"
	"github.com/gorilla/websocket"
	"github.com/276793422/NemesisBot/module/logger"
)

// Session represents an active WebSocket client session
type Session struct {
	ID         string
	Conn       *websocket.Conn
	SenderID   string
	ChatID     string
	CreatedAt  time.Time
	LastActive time.Time
	mu         sync.Mutex
	sendQueue  *sendQueue // Thread-safe send queue
}

// SessionManager manages all active WebSocket sessions
type SessionManager struct {
	sessions    sync.Map // sessionID -> *Session
	timeout     time.Duration
	stopCleanup context.CancelFunc
}

// NewSessionManager creates a new session manager
func NewSessionManager(timeout time.Duration) *SessionManager {
	sm := &SessionManager{
		timeout: timeout,
	}
	sm.startCleanup()
	return sm
}

// CreateSession creates a new session with a unique ID
func (sm *SessionManager) CreateSession(conn *websocket.Conn) *Session {
	sessionID := sm.generateSessionID()
	senderID := fmt.Sprintf("web:%s", sessionID)
	chatID := senderID

	session := &Session{
		ID:         sessionID,
		Conn:       conn,
		SenderID:   senderID,
		ChatID:     chatID,
		CreatedAt:  time.Now(),
		LastActive: time.Now(),
	}

	sm.sessions.Store(sessionID, session)

	logger.DebugCF("web", "Session created", map[string]interface{}{
		"session_id": sessionID,
		"sender_id":  senderID,
		"chat_id":    chatID,
	})

	return session
}

// GetSession retrieves a session by ID
func (sm *SessionManager) GetSession(sessionID string) (*Session, bool) {
	value, ok := sm.sessions.Load(sessionID)
	if !ok {
		return nil, false
	}
	session, ok := value.(*Session)
	return session, ok
}

// RemoveSession removes a session
func (sm *SessionManager) RemoveSession(sessionID string) {
	sm.sessions.Delete(sessionID)
	logger.DebugCF("web", "Session removed", map[string]interface{}{
		"session_id": sessionID,
	})
}

// Broadcast sends a message to a specific session
func (sm *SessionManager) Broadcast(sessionID string, message []byte) error {
	session, ok := sm.GetSession(sessionID)
	if !ok {
		logger.WarnCF("web", "Session not found for broadcast",
			map[string]interface{}{
				"session_id": sessionID,
				"message_len": len(message),
			})
		return fmt.Errorf("session not found: %s", sessionID)
	}

	logger.DebugCF("web", "Broadcasting to session",
		map[string]interface{}{
			"session_id": sessionID,
			"message_len": len(message),
		})

	session.mu.Lock()
	defer session.mu.Unlock()

	// Update last active time
	session.LastActive = time.Now()

	// Check if connection is still active
	if session.Conn == nil {
		logger.ErrorCF("web", "Session connection is nil",
			map[string]interface{}{
				"session_id": sessionID,
			})
		return fmt.Errorf("session connection is nil")
	}

	// Use send queue if available (thread-safe), otherwise direct send (legacy)
	if session.sendQueue != nil {
		err := session.sendQueue.send(websocket.TextMessage, message)
		if err != nil {
			logger.ErrorCF("web", "Failed to send via queue",
				map[string]interface{}{
					"session_id": sessionID,
					"error": err.Error(),
				})
			return fmt.Errorf("failed to send via queue: %w", err)
		}
		logger.DebugCF("web", "Message sent via queue",
			map[string]interface{}{
				"session_id": sessionID,
				"message_len": len(message),
			})
		return nil
	}

	// Legacy fallback: direct send (not recommended for concurrent use)
	logger.DebugCF("web", "Using legacy direct send",
		map[string]interface{}{
			"session_id": sessionID,
		})

	err := session.Conn.SetWriteDeadline(time.Now().Add(10 * time.Second))
	if err != nil {
		logger.ErrorCF("web", "Failed to set write deadline",
			map[string]interface{}{
				"session_id": sessionID,
				"error": err.Error(),
			})
		return fmt.Errorf("failed to set write deadline: %w", err)
	}

	err = session.Conn.WriteMessage(websocket.TextMessage, message)
	if err != nil {
		logger.ErrorCF("web", "Failed to write message",
			map[string]interface{}{
				"session_id": sessionID,
				"error": err.Error(),
			})
		return fmt.Errorf("failed to write message: %w", err)
	}

	logger.DebugCF("web", "Message sent successfully",
		map[string]interface{}{
			"session_id": sessionID,
			"message_len": len(message),
		})

	return nil
}

// Stats returns statistics about active sessions
func (sm *SessionManager) Stats() map[string]interface{} {
	count := 0
	sm.sessions.Range(func(_, _ interface{}) bool {
		count++
		return true
	})

	return map[string]interface{}{
		"active_sessions": count,
	}
}

// GetActiveCount returns the number of active sessions
func (sm *SessionManager) GetActiveCount() int {
	count := 0
	sm.sessions.Range(func(_, _ interface{}) bool {
		count++
		return true
	})
	return count
}

// GetAllSessions returns all active sessions
func (sm *SessionManager) GetAllSessions() []*Session {
	var sessions []*Session
	sm.sessions.Range(func(_, value interface{}) bool {
		if session, ok := value.(*Session); ok {
			sessions = append(sessions, session)
		}
		return true
	})
	return sessions
}

// Shutdown closes all sessions and stops cleanup
func (sm *SessionManager) Shutdown() {
	// Stop cleanup goroutine
	if sm.stopCleanup != nil {
		sm.stopCleanup()
	}

	// Close all sessions
	var wg sync.WaitGroup
	sm.sessions.Range(func(key, value interface{}) bool {
		wg.Add(1)
		go func(sessionID string, session *Session) {
			defer wg.Done()
			session.mu.Lock()
			defer session.mu.Unlock()
			if session.Conn != nil {
				_ = session.Conn.Close()
			}
			sm.sessions.Delete(sessionID)
		}(key.(string), value.(*Session))
		return true
	})
	wg.Wait()

	logger.InfoC("web", "Session manager shutdown complete")
}

// startCleanup starts a background goroutine to clean up inactive sessions
func (sm *SessionManager) startCleanup() {
	ctx, cancel := context.WithCancel(context.Background())
	sm.stopCleanup = cancel

	go func() {
		ticker := time.NewTicker(5 * time.Minute)
		defer ticker.Stop()

		for {
			select {
			case <-ctx.Done():
				logger.DebugC("web", "Session cleanup goroutine stopped")
				return
			case <-ticker.C:
				sm.cleanupInactiveSessions()
			}
		}
	}()

	logger.DebugC("web", "Session cleanup goroutine started")
}

// cleanupInactiveSessions removes sessions that have been inactive for too long
func (sm *SessionManager) cleanupInactiveSessions() {
	now := time.Now()
	var toRemove []string

	sm.sessions.Range(func(key, value interface{}) bool {
		sessionID := key.(string)
		session := value.(*Session)

		session.mu.Lock()
		inactive := now.Sub(session.LastActive) > sm.timeout
		session.mu.Unlock()

		if inactive {
			toRemove = append(toRemove, sessionID)
		}
		return true
	})

	for _, sessionID := range toRemove {
		session, ok := sm.GetSession(sessionID)
		if ok {
			logger.InfoCF("web", "Removing inactive session", map[string]interface{}{
				"session_id": sessionID,
				"inactive_duration": time.Since(session.LastActive).String(),
			})
			session.mu.Lock()
			if session.Conn != nil {
				_ = session.Conn.Close()
			}
			session.mu.Unlock()
			sm.RemoveSession(sessionID)
		}
	}

	if len(toRemove) > 0 {
		logger.InfoCF("web", "Cleaned up inactive sessions", map[string]interface{}{
			"count": len(toRemove),
		})
	}
}

// generateSessionID generates a unique session ID
func (sm *SessionManager) generateSessionID() string {
	// Generate UUID and remove dashes
	id := uuid.New().String()
	id = strings.ReplaceAll(id, "-", "")
	return id[:16] // Use first 16 characters for shorter ID
}
