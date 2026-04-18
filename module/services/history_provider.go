// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
// Services - Session History Provider for Web Chat

package services

import (
	"github.com/276793422/NemesisBot/module/session"
	"github.com/276793422/NemesisBot/module/web"
)

// SessionHistoryProvider implements web.HistoryProvider using a session manager.
type SessionHistoryProvider struct {
	sessions   *session.SessionManager
	sessionKey string
}

// NewSessionHistoryProvider creates a new history provider bound to a specific session key.
func NewSessionHistoryProvider(sessions *session.SessionManager, sessionKey string) *SessionHistoryProvider {
	return &SessionHistoryProvider{
		sessions:   sessions,
		sessionKey: sessionKey,
	}
}

// GetHistory returns a page of chat history from the session manager.
// It filters to only user and assistant messages and supports cursor-based pagination.
func (p *SessionHistoryProvider) GetHistory(limit int, beforeIndex *int) (*web.HistoryPage, error) {
	allMsgs := p.sessions.GetHistory(p.sessionKey)

	// Filter: only keep user and assistant messages
	filtered := make([]web.HistoryMessage, 0, len(allMsgs))
	for _, msg := range allMsgs {
		if msg.Role == "user" || msg.Role == "assistant" {
			filtered = append(filtered, web.HistoryMessage{
				Role:    msg.Role,
				Content: msg.Content,
			})
		}
	}

	totalCount := len(filtered)

	// Determine the end boundary
	end := totalCount
	if beforeIndex != nil && *beforeIndex >= 0 && *beforeIndex < totalCount {
		end = *beforeIndex
	}

	// Determine the start boundary
	start := end - limit
	if start < 0 {
		start = 0
	}

	hasMore := start > 0
	oldestIndex := start

	page := &web.HistoryPage{
		HasMore:     hasMore,
		OldestIndex: oldestIndex,
		TotalCount:  totalCount,
	}

	if start < end {
		page.Messages = filtered[start:end]
	} else {
		page.Messages = []web.HistoryMessage{}
	}

	return page, nil
}
