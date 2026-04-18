// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
// Web Channel Module - History Types and Provider Interface

package web

// HistoryMessage represents a single message in chat history.
type HistoryMessage struct {
	Role    string `json:"role"`
	Content string `json:"content"`
}

// HistoryPage represents a page of history messages returned to the client.
type HistoryPage struct {
	Messages    []HistoryMessage `json:"messages"`
	HasMore     bool             `json:"has_more"`
	OldestIndex int              `json:"oldest_index"`
	TotalCount  int              `json:"total_count"`
}

// HistoryRequestData represents the data payload of a history_request message.
type HistoryRequestData struct {
	RequestID   string `json:"request_id"`
	Limit       int    `json:"limit,omitempty"`
	BeforeIndex *int   `json:"before_index,omitempty"`
}
