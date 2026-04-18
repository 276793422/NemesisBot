// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package cluster

import (
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"time"
)

// ContinuationSnapshot 续行快照（存储到磁盘）
type ContinuationSnapshot struct {
	TaskID    string          `json:"task_id"`
	Messages  json.RawMessage `json:"messages"`      // []providers.Message 的原始 JSON
	ToolCallID string         `json:"tool_call_id"`  // 触发异步的 tool call ID
	Channel   string          `json:"channel"`       // 原始通道
	ChatID    string          `json:"chat_id"`       // 原始会话 ID
	CreatedAt time.Time       `json:"created_at"`    // 创建时间（用于清理）
}

// ContinuationStore 续行快照文件存储
type ContinuationStore struct {
	cacheDir string // {workspace}/cluster/rpc_cache/
}

// NewContinuationStore 创建续行存储
func NewContinuationStore(workspace string) (*ContinuationStore, error) {
	cacheDir := filepath.Join(workspace, "cluster", "rpc_cache")
	if err := os.MkdirAll(cacheDir, 0755); err != nil {
		return nil, fmt.Errorf("failed to create rpc_cache directory: %w", err)
	}
	return &ContinuationStore{cacheDir: cacheDir}, nil
}

// Save 保存续行快照到磁盘（原子写入：先写 tmp 再 rename）
func (s *ContinuationStore) Save(snapshot *ContinuationSnapshot) error {
	data, err := json.Marshal(snapshot)
	if err != nil {
		return fmt.Errorf("failed to marshal snapshot: %w", err)
	}
	filePath := filepath.Join(s.cacheDir, snapshot.TaskID+".json")
	tmpPath := filePath + ".tmp"
	if err := os.WriteFile(tmpPath, data, 0644); err != nil {
		return err
	}
	return os.Rename(tmpPath, filePath)
}

// Load 从磁盘加载续行快照
func (s *ContinuationStore) Load(taskID string) (*ContinuationSnapshot, error) {
	filePath := filepath.Join(s.cacheDir, taskID+".json")
	data, err := os.ReadFile(filePath)
	if err != nil {
		return nil, fmt.Errorf("snapshot not found: %w", err)
	}
	var snapshot ContinuationSnapshot
	if err := json.Unmarshal(data, &snapshot); err != nil {
		return nil, fmt.Errorf("failed to unmarshal snapshot: %w", err)
	}
	return &snapshot, nil
}

// Delete 删除续行快照文件
func (s *ContinuationStore) Delete(taskID string) error {
	filePath := filepath.Join(s.cacheDir, taskID+".json")
	return os.Remove(filePath)
}

// CleanupOld 清理超过 maxAge 的快照文件，返回清理数量
func (s *ContinuationStore) CleanupOld(maxAge time.Duration) (int, error) {
	entries, err := os.ReadDir(s.cacheDir)
	if err != nil {
		return 0, fmt.Errorf("failed to read rpc_cache directory: %w", err)
	}

	cleaned := 0
	cutoff := time.Now().Add(-maxAge)

	for _, entry := range entries {
		ext := filepath.Ext(entry.Name())
		if entry.IsDir() || (ext != ".json" && ext != ".tmp") {
			continue
		}

		info, err := entry.Info()
		if err != nil {
			continue
		}

		if info.ModTime().Before(cutoff) {
			filePath := filepath.Join(s.cacheDir, entry.Name())
			if err := os.Remove(filePath); err == nil {
				cleaned++
			}
		}
	}

	return cleaned, nil
}

// ListPending 列出所有待处理的快照 taskID（用于启动时恢复）
func (s *ContinuationStore) ListPending() ([]string, error) {
	entries, err := os.ReadDir(s.cacheDir)
	if err != nil {
		return nil, fmt.Errorf("failed to read rpc_cache directory: %w", err)
	}

	var taskIDs []string
	for _, entry := range entries {
		if entry.IsDir() {
			continue
		}
		name := entry.Name()
		if filepath.Ext(name) != ".json" {
			continue
		}
		taskID := name[:len(name)-len(".json")]
		taskIDs = append(taskIDs, taskID)
	}

	return taskIDs, nil
}
