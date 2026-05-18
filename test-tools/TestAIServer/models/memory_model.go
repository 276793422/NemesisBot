package models

import (
	"encoding/json"
	"fmt"
	"regexp"
	"strings"
	"time"
)

// TestAI60 - Memory tools test model
// Recognizes memory-related patterns and returns appropriate tool_calls.
//
// Chinese patterns (for UTF-8 capable clients):
//   - "记住：X" / "记住:X" → memory_store
//   - "关于 Y 你知道什么" / "搜索记忆 Y" → memory_search
//   - "列出记忆" / "列出所有记忆" → memory_list
//
// English patterns (encoding-safe fallback):
//   - "STORE: X" / "store: X" → memory_store
//   - "SEARCH: Y" / "search: Y" → memory_search
//   - "LIST MEMORY" / "list memory" → memory_list
//
// Default: "好的，我知道了"
type TestAI60 struct{}

func NewTestAI60() *TestAI60 { return &TestAI60{} }

func (m *TestAI60) Name() string { return "testai-6.0" }

func (m *TestAI60) Delay() time.Duration { return 0 }

// Regex patterns — both Chinese and English
var (
	// Store patterns
	storePatternCN = regexp.MustCompile(`(?i)记住[:：]\s*(.+)`)
	storePatternEN = regexp.MustCompile(`(?i)STORE:\s*(.+)`)

	// Search patterns
	searchPatternCN1 = regexp.MustCompile(`(?i)关于\s*(.+?)\s*你知道什么`)
	searchPatternCN2 = regexp.MustCompile(`(?i)搜索记忆\s*(.+)`)
	searchPatternEN  = regexp.MustCompile(`(?i)SEARCH:\s*(.+)`)

	// List patterns
	listPatternCN = regexp.MustCompile(`(?i)列出(?:所有)?记忆`)
	listPatternEN = regexp.MustCompile(`(?i)LIST\s+MEMORY`)
)

func (m *TestAI60) Process(messages []Message) string {
	if len(messages) == 0 {
		return "好的，我知道了"
	}

	lastMsgObj := messages[len(messages)-1]
	content := lastMsgObj.Content

	// Handle tool result (second round)
	if lastMsgObj.Role == "tool" {
		return fmt.Sprintf("Memory operation completed: %s", truncateStr(content, 200))
	}

	toolCallID := fmt.Sprintf("call-%d", time.Now().UnixNano())

	// Pattern 1: memory_store
	if matches := storePatternCN.FindStringSubmatch(content); len(matches) > 1 {
		return m.buildStoreToolCall(strings.TrimSpace(matches[1]), toolCallID)
	}
	if matches := storePatternEN.FindStringSubmatch(content); len(matches) > 1 {
		return m.buildStoreToolCall(strings.TrimSpace(matches[1]), toolCallID)
	}

	// Pattern 2: memory_search
	if matches := searchPatternCN1.FindStringSubmatch(content); len(matches) > 1 {
		return m.buildSearchToolCall(strings.TrimSpace(matches[1]), toolCallID)
	}
	if matches := searchPatternCN2.FindStringSubmatch(content); len(matches) > 1 {
		return m.buildSearchToolCall(strings.TrimSpace(matches[1]), toolCallID)
	}
	if matches := searchPatternEN.FindStringSubmatch(content); len(matches) > 1 {
		return m.buildSearchToolCall(strings.TrimSpace(matches[1]), toolCallID)
	}

	// Pattern 3: memory_list
	if listPatternCN.MatchString(content) || listPatternEN.MatchString(content) {
		return m.buildListToolCall(toolCallID)
	}

	// Default response
	return "好的，我知道了"
}

func (m *TestAI60) buildStoreToolCall(text, id string) string {
	args, _ := json.Marshal(map[string]interface{}{
		"memory_type": "episodic",
		"content":     text,
		"tags":        []string{"user-requested"},
	})
	resp, _ := json.Marshal(ProcessedResponse{
		ToolCalls: []ToolCall{{
			ID:   id,
			Type: "function",
			Function: &FunctionCall{
				Name:      "memory_store",
				Arguments: string(args),
			},
		}},
	})
	return string(resp)
}

func (m *TestAI60) buildSearchToolCall(query, id string) string {
	args, _ := json.Marshal(map[string]interface{}{
		"query": query,
		"limit": 10,
	})
	resp, _ := json.Marshal(ProcessedResponse{
		ToolCalls: []ToolCall{{
			ID:   id,
			Type: "function",
			Function: &FunctionCall{
				Name:      "memory_search",
				Arguments: string(args),
			},
		}},
	})
	return string(resp)
}

func (m *TestAI60) buildListToolCall(id string) string {
	args, _ := json.Marshal(map[string]interface{}{
		"list_type": "status",
	})
	resp, _ := json.Marshal(ProcessedResponse{
		ToolCalls: []ToolCall{{
			ID:   id,
			Type: "function",
			Function: &FunctionCall{
				Name:      "memory_list",
				Arguments: string(args),
			},
		}},
	})
	return string(resp)
}

func truncateStr(s string, maxLen int) string {
	// Count runes, not bytes
	runes := []rune(s)
	if len(runes) <= maxLen {
		return s
	}
	return string(runes[:maxLen]) + "..."
}
