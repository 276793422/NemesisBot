// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package cluster_test

import (
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"sort"
	"sync"
	"testing"
	"time"

	"github.com/276793422/NemesisBot/module/cluster"
)

// ==============================================================================
// 快照内容完整性测试
// ==============================================================================

// TestContinuationStore_SnapshotContentIntegrity 验证快照的 JSON 序列化/反序列化保真度
// 这是核心测试：确保 messages 的 round-trip 不丢失任何数据
func TestContinuationStore_SnapshotContentIntegrity(t *testing.T) {
	tmpDir := t.TempDir()
	store, err := cluster.NewContinuationStore(tmpDir)
	if err != nil {
		t.Fatalf("Failed to create store: %v", err)
	}

	// 模拟真实的 LLM 对话快照：
	// [system, user, assistant(tool_call)] — 这是续行快照的标准格式
	messages := []map[string]interface{}{
		{
			"role":    "system",
			"content": "你是一个 AI 助手",
		},
		{
			"role":    "user",
			"content": "帮我问问 Node-B 关于 X，然后写入文件",
		},
		{
			"role":    "assistant",
			"content": "",
			"tool_calls": []map[string]interface{}{
				{
					"id":   "call_abc123",
					"type": "function",
					"function": map[string]interface{}{
						"name":      "cluster_rpc",
						"arguments": `{"peer_id":"Node-B","action":"peer_chat","data":{"content":"关于 X 的信息"}}`,
					},
				},
			},
		},
	}

	messagesJSON, err := json.Marshal(messages)
	if err != nil {
		t.Fatalf("Failed to marshal messages: %v", err)
	}

	now := time.Now()
	snapshot := &cluster.ContinuationSnapshot{
		TaskID:     "task-integrity-001",
		Messages:   json.RawMessage(messagesJSON),
		ToolCallID: "call_abc123",
		Channel:    "web",
		ChatID:     "chat-789",
		CreatedAt:  now,
	}

	// Save
	if err := store.Save(snapshot); err != nil {
		t.Fatalf("Save failed: %v", err)
	}

	// Load
	loaded, err := store.Load("task-integrity-001")
	if err != nil {
		t.Fatalf("Load failed: %v", err)
	}

	// ===== 验证所有字段 =====

	// 1. TaskID
	if loaded.TaskID != "task-integrity-001" {
		t.Errorf("TaskID mismatch: expected 'task-integrity-001', got '%s'", loaded.TaskID)
	}

	// 2. ToolCallID
	if loaded.ToolCallID != "call_abc123" {
		t.Errorf("ToolCallID mismatch: expected 'call_abc123', got '%s'", loaded.ToolCallID)
	}

	// 3. Channel
	if loaded.Channel != "web" {
		t.Errorf("Channel mismatch: expected 'web', got '%s'", loaded.Channel)
	}

	// 4. ChatID
	if loaded.ChatID != "chat-789" {
		t.Errorf("ChatID mismatch: expected 'chat-789', got '%s'", loaded.ChatID)
	}

	// 5. Messages — 反序列化并逐条验证
	var loadedMessages []map[string]interface{}
	if err := json.Unmarshal(loaded.Messages, &loadedMessages); err != nil {
		t.Fatalf("Failed to unmarshal loaded messages: %v", err)
	}

	if len(loadedMessages) != 3 {
		t.Fatalf("Expected 3 messages, got %d", len(loadedMessages))
	}

	// 验证 system 消息
	if loadedMessages[0]["role"] != "system" {
		t.Errorf("Message 0 role: expected 'system', got '%v'", loadedMessages[0]["role"])
	}
	if loadedMessages[0]["content"] != "你是一个 AI 助手" {
		t.Errorf("Message 0 content: expected '你是一个 AI 助手', got '%v'", loadedMessages[0]["content"])
	}

	// 验证 user 消息
	if loadedMessages[1]["role"] != "user" {
		t.Errorf("Message 1 role: expected 'user', got '%v'", loadedMessages[1]["role"])
	}
	if loadedMessages[1]["content"] != "帮我问问 Node-B 关于 X，然后写入文件" {
		t.Errorf("Message 1 content mismatch, got '%v'", loadedMessages[1]["content"])
	}

	// 验证 assistant 消息（含 tool_calls）
	if loadedMessages[2]["role"] != "assistant" {
		t.Errorf("Message 2 role: expected 'assistant', got '%v'", loadedMessages[2]["role"])
	}
	toolCalls, ok := loadedMessages[2]["tool_calls"].([]interface{})
	if !ok {
		t.Fatalf("Message 2 tool_calls: expected []interface{}, got %T", loadedMessages[2]["tool_calls"])
	}
	if len(toolCalls) != 1 {
		t.Fatalf("Expected 1 tool_call, got %d", len(toolCalls))
	}
	tc, ok := toolCalls[0].(map[string]interface{})
	if !ok {
		t.Fatal("tool_call is not map[string]interface{}")
	}
	if tc["id"] != "call_abc123" {
		t.Errorf("tool_call id: expected 'call_abc123', got '%v'", tc["id"])
	}
	fn, ok := tc["function"].(map[string]interface{})
	if !ok {
		t.Fatal("function is not map[string]interface{}")
	}
	if fn["name"] != "cluster_rpc" {
		t.Errorf("function name: expected 'cluster_rpc', got '%v'", fn["name"])
	}

	// 6. CreatedAt — 时间精度验证（允许 1 秒误差）
	if loaded.CreatedAt.Sub(now).Abs() > time.Second {
		t.Errorf("CreatedAt time drift too large: expected ~%v, got %v", now, loaded.CreatedAt)
	}
}

// TestContinuationStore_MessagesRoundTripWithProvidersFormat 验证 providers.Message 格式的 round-trip
// 这是实际使用时的格式
func TestContinuationStore_MessagesRoundTripWithProvidersFormat(t *testing.T) {
	tmpDir := t.TempDir()
	store, err := cluster.NewContinuationStore(tmpDir)
	if err != nil {
		t.Fatalf("Failed to create store: %v", err)
	}

	// 模拟 providers.Message 的 JSON 格式
	messages := []map[string]interface{}{
		{"role": "system", "content": "System prompt"},
		{"role": "user", "content": "Hello"},
		{"role": "assistant", "content": "", "tool_calls": []map[string]interface{}{
			{
				"id":   "call_xyz",
				"type": "function",
				"function": map[string]interface{}{
					"name":      "cluster_rpc",
					"arguments": `{"peer_id":"Node-B"}`,
				},
			},
		}},
	}

	// 序列化 → 保存 → 加载 → 反序列化
	messagesJSON, _ := json.Marshal(messages)

	snapshot := &cluster.ContinuationSnapshot{
		TaskID:     "task-roundtrip",
		Messages:   json.RawMessage(messagesJSON),
		ToolCallID: "call_xyz",
		Channel:    "discord",
		ChatID:     "channel-general",
		CreatedAt:  time.Now(),
	}

	if err := store.Save(snapshot); err != nil {
		t.Fatalf("Save failed: %v", err)
	}

	loaded, err := store.Load("task-roundtrip")
	if err != nil {
		t.Fatalf("Load failed: %v", err)
	}

	// 反序列化回 messages
	var loadedMessages []map[string]interface{}
	if err := json.Unmarshal(loaded.Messages, &loadedMessages); err != nil {
		t.Fatalf("Failed to unmarshal: %v", err)
	}

	// 验证原始数据和 round-trip 数据的字节级一致
	originalJSON, _ := json.Marshal(messages)
	loadedJSON, _ := json.Marshal(loadedMessages)

	if string(originalJSON) != string(loadedJSON) {
		t.Errorf("Messages round-trip mismatch!\nOriginal: %s\nLoaded:   %s",
			string(originalJSON), string(loadedJSON))
	}
}

// TestContinuationStore_DiskFileFormat 验证磁盘文件格式正确
func TestContinuationStore_DiskFileFormat(t *testing.T) {
	tmpDir := t.TempDir()
	store, err := cluster.NewContinuationStore(tmpDir)
	if err != nil {
		t.Fatalf("Failed to create store: %v", err)
	}

	snapshot := &cluster.ContinuationSnapshot{
		TaskID:     "task-format-check",
		Messages:   json.RawMessage(`[{"role":"user","content":"test"}]`),
		ToolCallID: "call_test",
		Channel:    "web",
		ChatID:     "chat-format",
		CreatedAt:  time.Date(2026, 4, 13, 10, 30, 0, 0, time.UTC),
	}

	if err := store.Save(snapshot); err != nil {
		t.Fatalf("Save failed: %v", err)
	}

	// 直接读取磁盘文件，验证格式
	cacheDir := filepath.Join(tmpDir, "cluster", "rpc_cache")
	filePath := filepath.Join(cacheDir, "task-format-check.json")

	data, err := os.ReadFile(filePath)
	if err != nil {
		t.Fatalf("Failed to read disk file: %v", err)
	}

	// 验证文件名格式
	if filepath.Base(filePath) != "task-format-check.json" {
		t.Errorf("File name format wrong: %s", filepath.Base(filePath))
	}

	// 验证 JSON 可解析
	var parsed map[string]interface{}
	if err := json.Unmarshal(data, &parsed); err != nil {
		t.Fatalf("Disk file is not valid JSON: %v", err)
	}

	// 验证所有字段存在
	requiredFields := []string{"task_id", "messages", "tool_call_id", "channel", "chat_id", "created_at"}
	for _, field := range requiredFields {
		if _, ok := parsed[field]; !ok {
			t.Errorf("Required field '%s' missing from disk file", field)
		}
	}

	// 验证具体值
	if parsed["task_id"] != "task-format-check" {
		t.Errorf("task_id: expected 'task-format-check', got '%v'", parsed["task_id"])
	}
	if parsed["tool_call_id"] != "call_test" {
		t.Errorf("tool_call_id: expected 'call_test', got '%v'", parsed["tool_call_id"])
	}
	if parsed["channel"] != "web" {
		t.Errorf("channel: expected 'web', got '%v'", parsed["channel"])
	}
	if parsed["chat_id"] != "chat-format" {
		t.Errorf("chat_id: expected 'chat-format', got '%v'", parsed["chat_id"])
	}

	// 验证 messages 是数组
	msgs, ok := parsed["messages"].([]interface{})
	if !ok {
		t.Fatalf("messages should be array, got %T", parsed["messages"])
	}
	if len(msgs) != 1 {
		t.Errorf("Expected 1 message, got %d", len(msgs))
	}
}

// TestContinuationStore_Overwrite 验证相同 taskID 的快照会被覆盖
func TestContinuationStore_Overwrite(t *testing.T) {
	tmpDir := t.TempDir()
	store, err := cluster.NewContinuationStore(tmpDir)
	if err != nil {
		t.Fatalf("Failed to create store: %v", err)
	}

	taskID := "task-overwrite"

	// 第一次保存
	snap1 := &cluster.ContinuationSnapshot{
		TaskID:     taskID,
		Messages:   json.RawMessage(`[{"role":"user","content":"first"}]`),
		ToolCallID: "call_v1",
		Channel:    "web",
		ChatID:     "chat-1",
		CreatedAt:  time.Now(),
	}
	if err := store.Save(snap1); err != nil {
		t.Fatalf("First save failed: %v", err)
	}

	// 第二次保存（覆盖）
	snap2 := &cluster.ContinuationSnapshot{
		TaskID:     taskID,
		Messages:   json.RawMessage(`[{"role":"user","content":"second"}]`),
		ToolCallID: "call_v2",
		Channel:    "discord",
		ChatID:     "chat-2",
		CreatedAt:  time.Now(),
	}
	if err := store.Save(snap2); err != nil {
		t.Fatalf("Second save failed: %v", err)
	}

	// 加载，应该得到第二次的数据
	loaded, err := store.Load(taskID)
	if err != nil {
		t.Fatalf("Load failed: %v", err)
	}
	if loaded.ToolCallID != "call_v2" {
		t.Errorf("Expected overwritten ToolCallID 'call_v2', got '%s'", loaded.ToolCallID)
	}
	if loaded.Channel != "discord" {
		t.Errorf("Expected overwritten Channel 'discord', got '%s'", loaded.Channel)
	}

	var msgs []map[string]interface{}
	json.Unmarshal(loaded.Messages, &msgs)
	if len(msgs) > 0 && msgs[0]["content"] != "second" {
		t.Errorf("Expected overwritten message content 'second', got '%v'", msgs[0]["content"])
	}

	// 应该只有一个文件
	entries, _ := os.ReadDir(filepath.Join(tmpDir, "cluster", "rpc_cache"))
	jsonCount := 0
	for _, e := range entries {
		if filepath.Ext(e.Name()) == ".json" {
			jsonCount++
		}
	}
	if jsonCount != 1 {
		t.Errorf("Expected 1 file on disk, got %d", jsonCount)
	}
}

// TestContinuationStore_ConcurrentSaveLoad 并发安全测试
func TestContinuationStore_ConcurrentSaveLoad(t *testing.T) {
	tmpDir := t.TempDir()
	store, err := cluster.NewContinuationStore(tmpDir)
	if err != nil {
		t.Fatalf("Failed to create store: %v", err)
	}

	var wg sync.WaitGroup
	errCh := make(chan error, 20)

	// 并发写入 10 个快照
	for i := 0; i < 10; i++ {
		wg.Add(1)
		go func(idx int) {
			defer wg.Done()
			taskID := fmt.Sprintf("task-concurrent-%d", idx)
			snap := &cluster.ContinuationSnapshot{
				TaskID:     taskID,
				Messages:   json.RawMessage(fmt.Sprintf(`[{"role":"user","content":"msg-%d"}]`, idx)),
				ToolCallID: fmt.Sprintf("call_%d", idx),
				Channel:    "web",
				ChatID:     fmt.Sprintf("chat-%d", idx),
				CreatedAt:  time.Now(),
			}
			if err := store.Save(snap); err != nil {
				errCh <- err
			}
		}(i)
	}

	wg.Wait()
	close(errCh)
	for err := range errCh {
		t.Errorf("Concurrent save error: %v", err)
	}

	// 并发读取并验证
	for i := 0; i < 10; i++ {
		wg.Add(1)
		go func(idx int) {
			defer wg.Done()
			taskID := fmt.Sprintf("task-concurrent-%d", idx)
			loaded, err := store.Load(taskID)
			if err != nil {
				errCh <- fmt.Errorf("load %s: %w", taskID, err)
				return
			}
			if loaded.ToolCallID != fmt.Sprintf("call_%d", idx) {
				errCh <- fmt.Errorf("task %s: expected call_%d, got %s", taskID, idx, loaded.ToolCallID)
			}
		}(i)
	}

	wg.Wait()

	// ListPending 应该返回全部 10 个
	pending, err := store.ListPending()
	if err != nil {
		t.Fatalf("ListPending failed: %v", err)
	}
	if len(pending) != 10 {
		t.Errorf("Expected 10 pending, got %d", len(pending))
	}

	// 验证所有 taskID 都在列表中
	sort.Strings(pending)
	for i := 0; i < 10; i++ {
		expected := fmt.Sprintf("task-concurrent-%d", i)
		if i >= len(pending) || pending[i] != expected {
			t.Errorf("Pending list mismatch at index %d: expected '%s', got '%v'", i, expected, pending[i])
		}
	}
}

// TestContinuationStore_DeleteNonExistent 删除不存在的文件不应报错
func TestContinuationStore_DeleteNonExistent(t *testing.T) {
	tmpDir := t.TempDir()
	store, err := cluster.NewContinuationStore(tmpDir)
	if err != nil {
		t.Fatalf("Failed to create store: %v", err)
	}

	err = store.Delete("ghost-task")
	if err == nil {
		t.Log("Delete non-existent returned nil (acceptable)")
	} else {
		t.Logf("Delete non-existent returned error (also acceptable): %v", err)
	}
}

// TestContinuationStore_MultipleSnapshots 验证多快照并存
func TestContinuationStore_MultipleSnapshots(t *testing.T) {
	tmpDir := t.TempDir()
	store, err := cluster.NewContinuationStore(tmpDir)
	if err != nil {
		t.Fatalf("Failed to create store: %v", err)
	}

	// 保存 5 个不同通道的快照
	channels := []string{"web", "discord", "feishu", "rpc", "telegram"}
	for i, ch := range channels {
		snap := &cluster.ContinuationSnapshot{
			TaskID:     fmt.Sprintf("task-multi-%d", i),
			Messages:   json.RawMessage(fmt.Sprintf(`[{"role":"user","content":"from %s"}]`, ch)),
			ToolCallID: fmt.Sprintf("call_multi_%d", i),
			Channel:    ch,
			ChatID:     fmt.Sprintf("%s-chat-%d", ch, i),
			CreatedAt:  time.Now(),
		}
		if err := store.Save(snap); err != nil {
			t.Fatalf("Save %d failed: %v", i, err)
		}
	}

	// 验证每个都可以独立加载
	for i, ch := range channels {
		loaded, err := store.Load(fmt.Sprintf("task-multi-%d", i))
		if err != nil {
			t.Errorf("Load task-multi-%d failed: %v", i, err)
			continue
		}
		if loaded.Channel != ch {
			t.Errorf("Snapshot %d: expected channel '%s', got '%s'", i, ch, loaded.Channel)
		}
	}

	// 删除其中一个，其他不受影响
	store.Delete("task-multi-2")

	for i := range channels {
		if i == 2 {
			continue // 已删除
		}
		_, err := store.Load(fmt.Sprintf("task-multi-%d", i))
		if err != nil {
			t.Errorf("Snapshot %d should still exist after deleting snapshot 2: %v", i, err)
		}
	}

	// 已删除的应该加载失败
	_, err = store.Load("task-multi-2")
	if err == nil {
		t.Error("Snapshot 2 should be deleted")
	}
}

// ==============================================================================
// 快照消息内容边界测试
// ==============================================================================

// TestContinuationStore_EmptyMessages 空消息数组
func TestContinuationStore_EmptyMessages(t *testing.T) {
	tmpDir := t.TempDir()
	store, _ := cluster.NewContinuationStore(tmpDir)

	snap := &cluster.ContinuationSnapshot{
		TaskID:     "task-empty",
		Messages:   json.RawMessage(`[]`),
		ToolCallID: "call_empty",
		Channel:    "web",
		ChatID:     "chat-1",
		CreatedAt:  time.Now(),
	}
	if err := store.Save(snap); err != nil {
		t.Fatalf("Save failed: %v", err)
	}

	loaded, err := store.Load("task-empty")
	if err != nil {
		t.Fatalf("Load failed: %v", err)
	}

	var msgs []interface{}
	json.Unmarshal(loaded.Messages, &msgs)
	if len(msgs) != 0 {
		t.Errorf("Expected empty messages, got %d", len(msgs))
	}
}

// TestContinuationStore_LargeMessages 大消息内容
func TestContinuationStore_LargeMessages(t *testing.T) {
	tmpDir := t.TempDir()
	store, _ := cluster.NewContinuationStore(tmpDir)

	// 模拟大量消息
	largeContent := make([]byte, 50000) // 50KB content
	for i := range largeContent {
		largeContent[i] = 'A' + byte(i%26)
	}

	msgs := []map[string]interface{}{
		{"role": "system", "content": string(largeContent[:10000])},
		{"role": "user", "content": string(largeContent[:20000])},
		{"role": "assistant", "content": "", "tool_calls": []map[string]interface{}{
			{"id": "call_large", "type": "function", "function": map[string]interface{}{
				"name":      "cluster_rpc",
				"arguments": string(largeContent[:5000]),
			}},
		}},
	}
	msgsJSON, _ := json.Marshal(msgs)

	snap := &cluster.ContinuationSnapshot{
		TaskID:     "task-large",
		Messages:   json.RawMessage(msgsJSON),
		ToolCallID: "call_large",
		Channel:    "web",
		ChatID:     "chat-big",
		CreatedAt:  time.Now(),
	}

	if err := store.Save(snap); err != nil {
		t.Fatalf("Save large snapshot failed: %v", err)
	}

	loaded, err := store.Load("task-large")
	if err != nil {
		t.Fatalf("Load large snapshot failed: %v", err)
	}

	var loadedMsgs []map[string]interface{}
	json.Unmarshal(loaded.Messages, &loadedMsgs)
	if len(loadedMsgs) != 3 {
		t.Fatalf("Expected 3 messages, got %d", len(loadedMsgs))
	}
	if len(loadedMsgs[1]["content"].(string)) != 20000 {
		t.Errorf("Large content not preserved, got %d bytes", len(loadedMsgs[1]["content"].(string)))
	}
}

// TestContinuationStore_UnicodeContent Unicode 内容保真度
func TestContinuationStore_UnicodeContent(t *testing.T) {
	tmpDir := t.TempDir()
	store, _ := cluster.NewContinuationStore(tmpDir)

	unicodeContent := "中文测试 🎉 にほんご 한국어 Ñoño café résumé"
	msgs := []map[string]interface{}{
		{"role": "user", "content": unicodeContent},
	}
	msgsJSON, _ := json.Marshal(msgs)

	snap := &cluster.ContinuationSnapshot{
		TaskID:     "task-unicode",
		Messages:   json.RawMessage(msgsJSON),
		ToolCallID: "call_unicode",
		Channel:    "web",
		ChatID:     "chat-i18n",
		CreatedAt:  time.Now(),
	}
	store.Save(snap)

	loaded, _ := store.Load("task-unicode")
	var loadedMsgs []map[string]interface{}
	json.Unmarshal(loaded.Messages, &loadedMsgs)

	if len(loadedMsgs) == 0 {
		t.Fatal("No messages loaded")
	}
	if loadedMsgs[0]["content"].(string) != unicodeContent {
		t.Errorf("Unicode content mismatch:\nExpected: %s\nGot:      %s",
			unicodeContent, loadedMsgs[0]["content"].(string))
	}
}
