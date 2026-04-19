// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package agent

import (
	"sync"
	"sync/atomic"
	"testing"
	"time"

	"github.com/276793422/NemesisBot/module/providers"
)

// --- Save Barrier Tests ---
// 验证 save barrier 机制在以下场景中的正确性：
// 1. 回调在 save 之前到达（竞态场景）
// 2. save 永远不发生（超时场景）
// 3. 并发 save 和 load
// 4. 磁盘回退路径

// TestSaveBarrier_LoadBeforeSave 验证：load 在 save 之前调用时，
// loadContinuation 会等待 save 完成后返回正确数据（save barrier 核心测试）
func TestSaveBarrier_LoadBeforeSave(t *testing.T) {
	loop := createTestAgentLoop(t)

	taskID := "barrier-test-1"
	messages := []providers.Message{
		{Role: "user", Content: "Hello"},
		{Role: "assistant", Content: "Hi there"},
	}

	var loadResult *continuationData
	var loadDone int32

	// 模拟：回调先到达（load 先执行）
	go func() {
		loadResult = loop.loadContinuation(taskID)
		atomic.StoreInt32(&loadDone, 1)
	}()

	// 等一小段时间确保 load 已经开始等待
	time.Sleep(50 * time.Millisecond)

	// 然后 save 发生（模拟 AgentLoop 的 saveContinuation）
	loop.saveContinuation(taskID, messages, "tc-barrier", "test-ch", "test-chat")

	// 等待 load 完成（最多 2 秒）
	deadline := time.Now().Add(2 * time.Second)
	for time.Now().Before(deadline) {
		if atomic.LoadInt32(&loadDone) == 1 {
			break
		}
		time.Sleep(10 * time.Millisecond)
	}

	if atomic.LoadInt32(&loadDone) == 0 {
		t.Fatal("loadContinuation did not complete within 2 seconds (deadlock?)")
	}

	if loadResult == nil {
		t.Fatal("loadContinuation returned nil after save completed")
	}

	if loadResult.toolCallID != "tc-barrier" {
		t.Errorf("Expected toolCallID 'tc-barrier', got '%s'", loadResult.toolCallID)
	}
	if loadResult.channel != "test-ch" {
		t.Errorf("Expected channel 'test-ch', got '%s'", loadResult.channel)
	}
	if loadResult.chatID != "test-chat" {
		t.Errorf("Expected chatID 'test-chat', got '%s'", loadResult.chatID)
	}
	if len(loadResult.messages) != 2 {
		t.Fatalf("Expected 2 messages, got %d", len(loadResult.messages))
	}
	if loadResult.messages[0].Content != "Hello" {
		t.Errorf("Expected first message 'Hello', got '%s'", loadResult.messages[0].Content)
	}

	t.Log("PASS: loadContinuation correctly waited for save to complete")
}

// TestSaveBarrier_LoadTimeout 验证：当 save 永远不发生时，
// loadContinuation 在超时后返回 nil（不会永久阻塞）
func TestSaveBarrier_LoadTimeout(t *testing.T) {
	loop := createTestAgentLoop(t)

	start := time.Now()
	data := loop.loadContinuation("never-saved-task")
	elapsed := time.Since(start)

	if data != nil {
		t.Error("Expected nil for task that was never saved")
	}

	// 应该在 5 秒超时后返回（允许一些误差）
	if elapsed < 4*time.Second {
		t.Errorf("loadContinuation returned too quickly: %v (expected ~5s timeout)", elapsed)
	}
	if elapsed > 7*time.Second {
		t.Errorf("loadContinuation took too long: %v (expected ~5s timeout)", elapsed)
	}

	t.Logf("PASS: loadContinuation correctly timed out after %v", elapsed)
}

// TestSaveBarrier_ConcurrentSaveAndLoad 验证：多个并发 load 和 save 不会死锁或数据错乱
func TestSaveBarrier_ConcurrentSaveAndLoad(t *testing.T) {
	loop := createTestAgentLoop(t)
	count := 10

	var wg sync.WaitGroup
	results := make([]*continuationData, count)
	errors := make([]error, count)

	// 先启动所有 load（模拟多个回调同时到达）
	for i := 0; i < count; i++ {
		wg.Add(1)
		go func(idx int) {
			defer wg.Done()
			taskID := taskIDForIndex(idx)
			data := loop.loadContinuation(taskID)
			results[idx] = data
		}(i)
	}

	// 短暂等待确保所有 load 都开始
	time.Sleep(50 * time.Millisecond)

	// 然后依次 save（模拟 AgentLoop 保存快照）
	for i := 0; i < count; i++ {
		taskID := taskIDForIndex(i)
		msg := []providers.Message{{Role: "user", Content: taskID}}
		loop.saveContinuation(taskID, msg, "tc-"+taskID, "ch", "chat")
	}

	// 等待所有 load 完成
	done := make(chan struct{})
	go func() {
		wg.Wait()
		close(done)
	}()

	select {
	case <-done:
		// OK
	case <-time.After(10 * time.Second):
		t.Fatal("Timed out waiting for concurrent loads to complete (deadlock?)")
	}

	// 验证结果
	for i := 0; i < count; i++ {
		if errors[i] != nil {
			t.Errorf("Load %d error: %v", i, errors[i])
			continue
		}
		if results[i] == nil {
			t.Errorf("Load %d returned nil", i)
			continue
		}
		taskID := taskIDForIndex(i)
		if results[i].messages[0].Content != taskID {
			t.Errorf("Load %d: expected content '%s', got '%s'",
				i, taskID, results[i].messages[0].Content)
		}
		if results[i].toolCallID != "tc-"+taskID {
			t.Errorf("Load %d: expected toolCallID 'tc-%s', got '%s'",
				i, taskID, results[i].toolCallID)
		}
	}

	t.Logf("PASS: %d concurrent save/load operations completed without deadlock", count)
}

// TestSaveBarrier_SaveThenLoad 验证：save 完成后 load 立即返回（无等待）
func TestSaveBarrier_SaveThenLoad(t *testing.T) {
	loop := createTestAgentLoop(t)

	messages := []providers.Message{{Role: "user", Content: "sync-test"}}

	// 先 save
	loop.saveContinuation("sync-task", messages, "tc-sync", "ch", "chat")

	// 后 load（应该立即返回，不阻塞）
	start := time.Now()
	data := loop.loadContinuation("sync-task")
	elapsed := time.Since(start)

	if data == nil {
		t.Fatal("Expected continuation data")
	}
	if elapsed > 100*time.Millisecond {
		t.Errorf("loadContinuation took too long after save: %v (expected immediate)", elapsed)
	}

	t.Logf("PASS: loadContinuation returned immediately (%v) after save", elapsed)
}

// TestSaveBarrier_SaveOverwrite 验证：同一 taskID 重复 save 时，
// 新的 ready channel 覆盖旧的，load 能拿到最新数据
func TestSaveBarrier_SaveOverwrite(t *testing.T) {
	loop := createTestAgentLoop(t)

	msg1 := []providers.Message{{Role: "user", Content: "first"}}
	msg2 := []providers.Message{{Role: "user", Content: "second"}}

	// 第一次 save
	loop.saveContinuation("overwrite-task", msg1, "tc-1", "ch", "chat-1")

	// 第二次 save（覆盖）
	loop.saveContinuation("overwrite-task", msg2, "tc-2", "ch", "chat-2")

	// load 应该拿到第二次的数据
	data := loop.loadContinuation("overwrite-task")
	if data == nil {
		t.Fatal("Expected continuation data")
	}
	if data.messages[0].Content != "second" {
		t.Errorf("Expected 'second' (latest save), got '%s'", data.messages[0].Content)
	}
	if data.toolCallID != "tc-2" {
		t.Errorf("Expected toolCallID 'tc-2', got '%s'", data.toolCallID)
	}

	t.Log("PASS: loadContinuation correctly returned latest save after overwrite")
}

// TestSaveBarrier_DeepCopyIntegrity 验证：save barrier 模式下的深拷贝仍然正确
func TestSaveBarrier_DeepCopyIntegrity(t *testing.T) {
	loop := createTestAgentLoop(t)

	messages := []providers.Message{{Role: "user", Content: "original"}}

	loop.saveContinuation("deepcopy-task", messages, "tc-dc", "ch", "chat")

	// 修改原始 slice
	messages[0].Content = "modified"

	data := loop.loadContinuation("deepcopy-task")
	if data == nil {
		t.Fatal("Expected continuation data")
	}
	if data.messages[0].Content != "original" {
		t.Errorf("Expected 'original' (deep copy), got '%s'", data.messages[0].Content)
	}

	t.Log("PASS: Deep copy integrity maintained with save barrier")
}

// taskIDForIndex generates a consistent task ID for concurrent tests
func taskIDForIndex(i int) string {
	return "concurrent-task-" + string(rune('A'+i))
}
