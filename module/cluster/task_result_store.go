// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package cluster

import (
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"sync"
	"time"
)

// TaskResultEntry B 端任务结果记录
type TaskResultEntry struct {
	TaskID       string    `json:"task_id"`
	Status       string    `json:"status"`        // "running" | "done"
	ResultStatus string    `json:"result_status"`  // "success" | "error"（仅 done 时有值）
	Response     string    `json:"response,omitempty"`
	Error        string    `json:"error,omitempty"`
	SourceNode   string    `json:"source_node"`
	CreatedAt    time.Time `json:"created_at"`
	UpdatedAt    time.Time `json:"updated_at"`
}

// TaskResultIndex 磁盘索引
type TaskResultIndex struct {
	Tasks map[string]*TaskResultEntry `json:"tasks"`
}

// TaskResultStore B 端任务结果持久化存储
// running 状态仅存内存（进程重启后丢失，A 端会再次询问）
// done 状态写磁盘（数据文件 + 索引文件）
type TaskResultStore struct {
	mu        sync.RWMutex
	dataDir   string // {workspace}/cluster/task_results/
	indexPath string // {workspace}/cluster/task_results/index.json
	running   map[string]bool       // 内存中的 running 状态追踪
	index     *TaskResultIndex      // 磁盘索引的内存缓存
}

// NewTaskResultStore 创建任务结果存储
func NewTaskResultStore(workspace string) (*TaskResultStore, error) {
	dataDir := filepath.Join(workspace, "cluster", "task_results")
	if err := os.MkdirAll(dataDir, 0755); err != nil {
		return nil, fmt.Errorf("failed to create task_results directory: %w", err)
	}

	s := &TaskResultStore{
		dataDir:   dataDir,
		indexPath: filepath.Join(dataDir, "index.json"),
		running:   make(map[string]bool),
		index:     &TaskResultIndex{Tasks: make(map[string]*TaskResultEntry)},
	}

	if err := s.loadIndex(); err != nil {
		// 索引加载失败不阻塞启动，从空索引开始
		s.index = &TaskResultIndex{Tasks: make(map[string]*TaskResultEntry)}
	}

	return s, nil
}

// SetRunning 标记任务为 running（仅内存）
func (s *TaskResultStore) SetRunning(taskID, sourceNode string) {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.running[taskID] = true
}

// SetResult 写入完成结果（数据文件 + 索引 + 磁盘），同时清理 running 标记
func (s *TaskResultStore) SetResult(taskID, resultStatus, response, errMsg, sourceNode string) error {
	s.mu.Lock()
	defer s.mu.Unlock()

	now := time.Now()
	entry := &TaskResultEntry{
		TaskID:       taskID,
		Status:       "done",
		ResultStatus: resultStatus,
		Response:     response,
		Error:        errMsg,
		SourceNode:   sourceNode,
		CreatedAt:    now,
		UpdatedAt:    now,
	}

	// 写数据文件（原子写入：tmp + rename）
	data, err := json.Marshal(entry)
	if err != nil {
		return fmt.Errorf("failed to marshal task result: %w", err)
	}
	filePath := filepath.Join(s.dataDir, taskID+".json")
	tmpPath := filePath + ".tmp"
	if err := os.WriteFile(tmpPath, data, 0644); err != nil {
		return err
	}
	if err := os.Rename(tmpPath, filePath); err != nil {
		return err
	}

	// 更新内存索引
	s.index.Tasks[taskID] = entry

	// 清理 running 标记
	delete(s.running, taskID)

	// 写索引到磁盘
	return s.saveIndexLocked()
}

// Get 查询任务结果（先查 running，再查索引）
func (s *TaskResultStore) Get(taskID string) *TaskResultEntry {
	s.mu.RLock()
	defer s.mu.RUnlock()

	// 先查 running
	if s.running[taskID] {
		return &TaskResultEntry{
			TaskID: taskID,
			Status: "running",
		}
	}

	// 再查索引
	if entry, ok := s.index.Tasks[taskID]; ok {
		return entry
	}

	return nil
}

// Delete 删除任务结果（数据文件 + 索引 + 磁盘）
func (s *TaskResultStore) Delete(taskID string) error {
	s.mu.Lock()
	defer s.mu.Unlock()

	// 删数据文件
	filePath := filepath.Join(s.dataDir, taskID+".json")
	os.Remove(filePath) // 忽略错误（文件可能不存在）

	// 更新索引
	delete(s.index.Tasks, taskID)
	delete(s.running, taskID)

	return s.saveIndexLocked()
}

// loadIndex 从磁盘加载索引
func (s *TaskResultStore) loadIndex() error {
	data, err := os.ReadFile(s.indexPath)
	if err != nil {
		if os.IsNotExist(err) {
			return nil // 索引文件不存在是正常的
		}
		return fmt.Errorf("failed to read task result index: %w", err)
	}

	var idx TaskResultIndex
	if err := json.Unmarshal(data, &idx); err != nil {
		return fmt.Errorf("failed to unmarshal task result index: %w", err)
	}

	if idx.Tasks == nil {
		idx.Tasks = make(map[string]*TaskResultEntry)
	}
	s.index = &idx
	return nil
}

// saveIndexLocked 将索引写入磁盘（调用方必须持有锁）
func (s *TaskResultStore) saveIndexLocked() error {
	data, err := json.Marshal(s.index)
	if err != nil {
		return fmt.Errorf("failed to marshal task result index: %w", err)
	}
	tmpPath := s.indexPath + ".tmp"
	if err := os.WriteFile(tmpPath, data, 0644); err != nil {
		return err
	}
	return os.Rename(tmpPath, s.indexPath)
}
