// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package cluster

import (
	"context"
	"fmt"
	"sync"
	"time"
)

// TaskStore 任务存储接口（Phase 2 可替换为持久化实现）
type TaskStore interface {
	Create(task *Task) error
	Get(taskID string) (*Task, error)
	UpdateResult(taskID string, result *TaskResult) error
	Delete(taskID string) error
	ListByStatus(status TaskStatus) ([]*Task, error)
}

// InMemoryTaskStore 内存实现（Phase 1）
type InMemoryTaskStore struct {
	mu    sync.RWMutex
	tasks map[string]*Task
}

// NewInMemoryTaskStore 创建内存任务存储
func NewInMemoryTaskStore() *InMemoryTaskStore {
	return &InMemoryTaskStore{
		tasks: make(map[string]*Task),
	}
}

// Create 创建任务记录
func (s *InMemoryTaskStore) Create(task *Task) error {
	s.mu.Lock()
	defer s.mu.Unlock()
	if _, exists := s.tasks[task.ID]; exists {
		return fmt.Errorf("task already exists: %s", task.ID)
	}
	s.tasks[task.ID] = task
	return nil
}

// Get 获取任务记录
func (s *InMemoryTaskStore) Get(taskID string) (*Task, error) {
	s.mu.RLock()
	defer s.mu.RUnlock()
	task, exists := s.tasks[taskID]
	if !exists {
		return nil, fmt.Errorf("task not found: %s", taskID)
	}
	return task, nil
}

// UpdateResult 更新任务结果
func (s *InMemoryTaskStore) UpdateResult(taskID string, result *TaskResult) error {
	s.mu.Lock()
	defer s.mu.Unlock()
	task, exists := s.tasks[taskID]
	if !exists {
		return fmt.Errorf("task not found: %s", taskID)
	}
	now := time.Now()
	task.CompletedAt = &now
	task.Response = result.Response
	task.Result = result.Result
	task.Error = result.Error

	switch result.Status {
	case "success":
		task.Status = TaskCompleted
	case "error":
		task.Status = TaskFailed
	default:
		task.Status = TaskFailed
	}
	return nil
}

// Delete 删除任务记录
func (s *InMemoryTaskStore) Delete(taskID string) error {
	s.mu.Lock()
	defer s.mu.Unlock()
	delete(s.tasks, taskID)
	return nil
}

// ListByStatus 按状态列出任务
func (s *InMemoryTaskStore) ListByStatus(status TaskStatus) ([]*Task, error) {
	s.mu.RLock()
	defer s.mu.RUnlock()
	var result []*Task
	for _, task := range s.tasks {
		if task.Status == status {
			result = append(result, task)
		}
	}
	return result, nil
}

// TaskManager 任务生命周期管理
type TaskManager struct {
	store           TaskStore
	cleanupInterval time.Duration // 清理间隔

	// Phase 1: 本地等待通道
	waitChs map[string]chan struct{} // taskID → done channel
	waitMu  sync.RWMutex

	stopCh chan struct{}
	wg     sync.WaitGroup
}

// NewTaskManager 创建任务管理器
func NewTaskManager(cleanupInterval time.Duration) *TaskManager {
	if cleanupInterval <= 0 {
		cleanupInterval = 30 * time.Second
	}
	return &TaskManager{
		store:           NewInMemoryTaskStore(),
		cleanupInterval: cleanupInterval,
		waitChs:         make(map[string]chan struct{}),
		stopCh:          make(chan struct{}),
	}
}

// Start 启动 TaskManager 的清理协程
func (tm *TaskManager) Start() {
	tm.wg.Add(1)
	go tm.cleanupLoop()
}

// Stop 停止 TaskManager
func (tm *TaskManager) Stop() {
	close(tm.stopCh)
	tm.wg.Wait()

	// 关闭所有等待 channel，通知可能阻塞的调用者
	tm.waitMu.Lock()
	for taskID, ch := range tm.waitChs {
		close(ch)
		delete(tm.waitChs, taskID)
	}
	tm.waitMu.Unlock()
}

// Submit 提交任务并创建本地等待 channel
func (tm *TaskManager) Submit(task *Task) error {
	if err := tm.store.Create(task); err != nil {
		return err
	}

	// 创建 done channel
	doneCh := make(chan struct{})
	tm.waitMu.Lock()
	tm.waitChs[task.ID] = doneCh
	tm.waitMu.Unlock()

	return nil
}

// WaitForTask 阻塞等待任务完成（Phase 1: 本地 channel）
func (tm *TaskManager) WaitForTask(ctx context.Context, taskID string) (*TaskResult, error) {
	// 获取 done channel
	tm.waitMu.RLock()
	doneCh, exists := tm.waitChs[taskID]
	tm.waitMu.RUnlock()

	if !exists {
		return nil, fmt.Errorf("task not found: %s", taskID)
	}

	// 等待完成或上下文取消
	select {
	case <-doneCh:
		// 任务完成，从 store 获取结果
		task, err := tm.store.Get(taskID)
		if err != nil {
			return nil, err
		}
		result := &TaskResult{
			TaskID:   taskID,
			Status:   string(task.Status),
			Response: task.Response,
		}
		if task.Result != nil {
			result.Result = task.Result
		}
		if task.Error != "" {
			result.Error = task.Error
		}
		return result, nil
	case <-ctx.Done():
		return nil, ctx.Err()
	case <-tm.stopCh:
		return nil, fmt.Errorf("task manager stopped")
	}
}

// CompleteTask 标记任务完成（由 CallbackHandler 调用）
func (tm *TaskManager) CompleteTask(taskID string, result *TaskResult) error {
	// 更新 store 中的任务状态和结果
	if err := tm.store.UpdateResult(taskID, result); err != nil {
		return err
	}

	// 关闭对应的 doneCh channel，通知 WaitForTask
	tm.waitMu.Lock()
	doneCh, exists := tm.waitChs[taskID]
	if exists {
		close(doneCh)
		delete(tm.waitChs, taskID)
	}
	tm.waitMu.Unlock()

	return nil
}

// GetTask 获取任务信息
func (tm *TaskManager) GetTask(taskID string) (*Task, error) {
	return tm.store.Get(taskID)
}

// CompleteCallback 实现 handlers.TaskCompleter 接口
// 将基本类型的回调参数转换为 TaskResult 后调用 CompleteTask
func (tm *TaskManager) CompleteCallback(taskID, status, response, errMsg string) error {
	result := &TaskResult{
		TaskID:   taskID,
		Status:   status,
		Response: response,
		Error:    errMsg,
	}
	return tm.CompleteTask(taskID, result)
}

// cleanupLoop 定期清理已完成的任务记录
func (tm *TaskManager) cleanupLoop() {
	defer tm.wg.Done()

	ticker := time.NewTicker(tm.cleanupInterval)
	defer ticker.Stop()

	for {
		select {
		case <-tm.stopCh:
			return
		case <-ticker.C:
			tm.cleanupCompleted()
		}
	}
}

// cleanupCompleted 清理已完成和已失败的任务记录
func (tm *TaskManager) cleanupCompleted() {
	statuses := []TaskStatus{TaskCompleted, TaskFailed, TaskCancelled}
	for _, status := range statuses {
		tasks, _ := tm.store.ListByStatus(status)
		for _, task := range tasks {
			// 只清理完成超过 5 分钟的任务
			if task.CompletedAt != nil && time.Since(*task.CompletedAt) > 5*time.Minute {
				tm.store.Delete(task.ID)
			}
		}
	}
}

// generateTaskID 生成唯一任务 ID
func generateTaskID() string {
	return fmt.Sprintf("task-%d", time.Now().UnixNano())
}
