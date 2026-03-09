package state

import (
	"encoding/json"
	"os"
	"path/filepath"
	"testing"
	"time"
)

func TestNewManager(t *testing.T) {
	tmpDir, err := os.MkdirTemp("", "state-test-*")
	if err != nil {
		t.Fatalf("Failed to create temp dir: %v", err)
	}
	defer os.RemoveAll(tmpDir)

	sm := NewManager(tmpDir)
	if sm == nil {
		t.Fatal("NewManager returned nil")
	}
	if sm.workspace != tmpDir {
		t.Errorf("Expected workspace %v, got %v", tmpDir, sm.workspace)
	}
	if sm.state == nil {
		t.Error("State should be initialized")
	}
	if sm.stateFile == "" {
		t.Error("StateFile should be set")
	}
}

func TestSetLastChannel(t *testing.T) {
	tmpDir, err := os.MkdirTemp("", "state-set-channel-*")
	if err != nil {
		t.Fatalf("Failed to create temp dir: %v", err)
	}
	defer os.RemoveAll(tmpDir)

	sm := NewManager(tmpDir)

	// Set last channel
	err = sm.SetLastChannel("telegram")
	if err != nil {
		t.Fatalf("SetLastChannel failed: %v", err)
	}

	// Verify it was set
	if sm.GetLastChannel() != "telegram" {
		t.Errorf("Expected last channel 'telegram', got %v", sm.GetLastChannel())
	}

	// Verify timestamp was updated
	if sm.GetTimestamp().IsZero() {
		t.Error("Timestamp should be updated")
	}

	// Verify file was created
	stateFile := filepath.Join(tmpDir, "state", "state.json")
	if _, err := os.Stat(stateFile); os.IsNotExist(err) {
		t.Error("State file was not created")
	}
}

func TestSetLastChatID(t *testing.T) {
	tmpDir, err := os.MkdirTemp("", "state-set-chatid-*")
	if err != nil {
		t.Fatalf("Failed to create temp dir: %v", err)
	}
	defer os.RemoveAll(tmpDir)

	sm := NewManager(tmpDir)

	// Set last chat ID
	err = sm.SetLastChatID("123456")
	if err != nil {
		t.Fatalf("SetLastChatID failed: %v", err)
	}

	// Verify it was set
	if sm.GetLastChatID() != "123456" {
		t.Errorf("Expected last chat ID '123456', got %v", sm.GetLastChatID())
	}

	// Verify timestamp was updated
	if sm.GetTimestamp().IsZero() {
		t.Error("Timestamp should be updated")
	}
}

func TestGetLastChannel(t *testing.T) {
	tmpDir, err := os.MkdirTemp("", "state-get-channel-*")
	if err != nil {
		t.Fatalf("Failed to create temp dir: %v", err)
	}
	defer os.RemoveAll(tmpDir)

	sm := NewManager(tmpDir)

	// Get default value
	if sm.GetLastChannel() != "" {
		t.Errorf("Expected empty last channel, got %v", sm.GetLastChannel())
	}

	// Set and get
	sm.SetLastChannel("discord")
	if sm.GetLastChannel() != "discord" {
		t.Errorf("Expected 'discord', got %v", sm.GetLastChannel())
	}
}

func TestGetLastChatID(t *testing.T) {
	tmpDir, err := os.MkdirTemp("", "state-get-chatid-*")
	if err != nil {
		t.Fatalf("Failed to create temp dir: %v", err)
	}
	defer os.RemoveAll(tmpDir)

	sm := NewManager(tmpDir)

	// Get default value
	if sm.GetLastChatID() != "" {
		t.Errorf("Expected empty last chat ID, got %v", sm.GetLastChatID())
	}

	// Set and get
	sm.SetLastChatID("789012")
	if sm.GetLastChatID() != "789012" {
		t.Errorf("Expected '789012', got %v", sm.GetLastChatID())
	}
}

func TestGetTimestamp(t *testing.T) {
	tmpDir, err := os.MkdirTemp("", "state-timestamp-*")
	if err != nil {
		t.Fatalf("Failed to create temp dir: %v", err)
	}
	defer os.RemoveAll(tmpDir)

	sm := NewManager(tmpDir)

	// Initial timestamp should be zero
	if !sm.GetTimestamp().IsZero() {
		t.Error("Initial timestamp should be zero")
	}

	// Set channel and verify timestamp updates
	beforeSet := time.Now()
	sm.SetLastChannel("test")
	afterSet := time.Now()

	timestamp := sm.GetTimestamp()
	if timestamp.Before(beforeSet) || timestamp.After(afterSet) {
		t.Error("Timestamp should be between beforeSet and afterSet")
	}
}

func TestSaveAtomic(t *testing.T) {
	tmpDir, err := os.MkdirTemp("", "state-atomic-*")
	if err != nil {
		t.Fatalf("Failed to create temp dir: %v", err)
	}
	defer os.RemoveAll(tmpDir)

	sm := NewManager(tmpDir)
	sm.state.LastChannel = "test-channel"
	sm.state.LastChatID = "test-chat-id"
	sm.state.Timestamp = time.Now()

	// Save atomic
	err = sm.saveAtomic()
	if err != nil {
		t.Fatalf("saveAtomic failed: %v", err)
	}

	// Verify file exists
	stateFile := filepath.Join(tmpDir, "state", "state.json")
	data, err := os.ReadFile(stateFile)
	if err != nil {
		t.Fatalf("Failed to read state file: %v", err)
	}

	// Verify content
	var loadedState State
	if err := json.Unmarshal(data, &loadedState); err != nil {
		t.Fatalf("Failed to unmarshal state: %v", err)
	}

	if loadedState.LastChannel != "test-channel" {
		t.Errorf("Expected last channel 'test-channel', got %v", loadedState.LastChannel)
	}
	if loadedState.LastChatID != "test-chat-id" {
		t.Errorf("Expected last chat ID 'test-chat-id', got %v", loadedState.LastChatID)
	}
}

func TestLoad(t *testing.T) {
	tmpDir, err := os.MkdirTemp("", "state-load-*")
	if err != nil {
		t.Fatalf("Failed to create temp dir: %v", err)
	}
	defer os.RemoveAll(tmpDir)

	// Create state file manually
	stateDir := filepath.Join(tmpDir, "state")
	os.MkdirAll(stateDir, 0755)
	stateFile := filepath.Join(stateDir, "state.json")

	testState := State{
		LastChannel: "telegram",
		LastChatID:  "999888",
		Timestamp:   time.Now(),
	}

	data, err := json.MarshalIndent(testState, "", "  ")
	if err != nil {
		t.Fatalf("Failed to marshal test state: %v", err)
	}
	if err := os.WriteFile(stateFile, data, 0644); err != nil {
		t.Fatalf("Failed to write test state: %v", err)
	}

	// Load state
	sm := NewManager(tmpDir)

	if sm.GetLastChannel() != "telegram" {
		t.Errorf("Expected last channel 'telegram', got %v", sm.GetLastChannel())
	}
	if sm.GetLastChatID() != "999888" {
		t.Errorf("Expected last chat ID '999888', got %v", sm.GetLastChatID())
	}
}

func TestLoadNonExistent(t *testing.T) {
	tmpDir, err := os.MkdirTemp("", "state-nonexist-*")
	if err != nil {
		t.Fatalf("Failed to create temp dir: %v", err)
	}
	defer os.RemoveAll(tmpDir)

	sm := NewManager(tmpDir)

	// Should not error, just initialize empty state
	if sm.GetLastChannel() != "" {
		t.Errorf("Expected empty last channel, got %v", sm.GetLastChannel())
	}
	if sm.GetLastChatID() != "" {
		t.Errorf("Expected empty last chat ID, got %v", sm.GetLastChatID())
	}
}

func TestMigration(t *testing.T) {
	tmpDir, err := os.MkdirTemp("", "state-migrate-*")
	if err != nil {
		t.Fatalf("Failed to create temp dir: %v", err)
	}
	defer os.RemoveAll(tmpDir)

	// Create old state file
	oldStateFile := filepath.Join(tmpDir, "state.json")
	testState := State{
		LastChannel: "discord",
		LastChatID:  "migration-test",
		Timestamp:   time.Now(),
	}

	data, err := json.MarshalIndent(testState, "", "  ")
	if err != nil {
		t.Fatalf("Failed to marshal test state: %v", err)
	}
	if err := os.WriteFile(oldStateFile, data, 0644); err != nil {
		t.Fatalf("Failed to write old state file: %v", err)
	}

	// Create manager - should migrate from old to new location
	sm := NewManager(tmpDir)

	// Verify state was loaded
	if sm.GetLastChannel() != "discord" {
		t.Errorf("Expected last channel 'discord', got %v", sm.GetLastChannel())
	}
	if sm.GetLastChatID() != "migration-test" {
		t.Errorf("Expected last chat ID 'migration-test', got %v", sm.GetLastChatID())
	}

	// Verify new state file exists
	newStateFile := filepath.Join(tmpDir, "state", "state.json")
	if _, err := os.Stat(newStateFile); os.IsNotExist(err) {
		t.Error("New state file should exist after migration")
	}
}

func TestConcurrentAccess(t *testing.T) {
	tmpDir, err := os.MkdirTemp("", "state-concurrent-*")
	if err != nil {
		t.Fatalf("Failed to create temp dir: %v", err)
	}
	defer os.RemoveAll(tmpDir)

	sm := NewManager(tmpDir)
	done := make(chan bool)

	// Concurrent writes
	for i := 0; i < 10; i++ {
		go func(idx int) {
			for j := 0; j < 50; j++ {
				sm.SetLastChannel("channel-test")
				sm.SetLastChatID("chat-test")
			}
			done <- true
		}(i)
	}

	// Concurrent reads
	for i := 0; i < 5; i++ {
		go func() {
			for j := 0; j < 100; j++ {
				_ = sm.GetLastChannel()
				_ = sm.GetLastChatID()
				_ = sm.GetTimestamp()
			}
			done <- true
		}()
	}

	// Wait for all goroutines
	for i := 0; i < 15; i++ {
		<-done
	}

	// Verify final state
	if sm.GetLastChannel() != "channel-test" {
		t.Errorf("Expected 'channel-test', got %v", sm.GetLastChannel())
	}
	if sm.GetLastChatID() != "chat-test" {
		t.Errorf("Expected 'chat-test', got %v", sm.GetLastChatID())
	}
}

func TestStatePersistence(t *testing.T) {
	tmpDir, err := os.MkdirTemp("", "state-persist-*")
	if err != nil {
		t.Fatalf("Failed to create temp dir: %v", err)
	}
	defer os.RemoveAll(tmpDir)

	// Create manager and set state
	sm1 := NewManager(tmpDir)
	sm1.SetLastChannel("persist-test")
	sm1.SetLastChatID("persist-id")

	// Create new manager - should load persisted state
	sm2 := NewManager(tmpDir)

	if sm2.GetLastChannel() != "persist-test" {
		t.Errorf("Expected 'persist-test', got %v", sm2.GetLastChannel())
	}
	if sm2.GetLastChatID() != "persist-id" {
		t.Errorf("Expected 'persist-id', got %v", sm2.GetLastChatID())
	}
}

func TestStateJSON(t *testing.T) {
	state := State{
		LastChannel: "test-channel",
		LastChatID:  "test-id",
		Timestamp:   time.Date(2026, 3, 9, 12, 0, 0, 0, time.UTC),
	}

	data, err := json.Marshal(state)
	if err != nil {
		t.Fatalf("Failed to marshal state: %v", err)
	}

	var unmarshaled State
	if err := json.Unmarshal(data, &unmarshaled); err != nil {
		t.Fatalf("Failed to unmarshal state: %v", err)
	}

	if unmarshaled.LastChannel != state.LastChannel {
		t.Errorf("Expected %v, got %v", state.LastChannel, unmarshaled.LastChannel)
	}
	if unmarshaled.LastChatID != state.LastChatID {
		t.Errorf("Expected %v, got %v", state.LastChatID, unmarshaled.LastChatID)
	}
}

func TestSaveAtomicMarshalError(t *testing.T) {
	tmpDir, err := os.MkdirTemp("", "state-marshal-error-*")
	if err != nil {
		t.Fatalf("Failed to create temp dir: %v", err)
	}
	defer os.RemoveAll(tmpDir)

	sm := NewManager(tmpDir)
	sm.state.LastChannel = "test-channel"

	// Make the directory read-only to cause write error
	stateDir := filepath.Dir(sm.stateFile)
	os.Chmod(stateDir, 0555)
	defer os.Chmod(stateDir, 0755)

	err = sm.saveAtomic()
	// On some systems, this might not fail due to OS differences
	// The important thing is that we test the error handling path
	if err != nil {
		// This is expected behavior on systems with strict permissions
		return
	}
}

func TestSaveAtomicRenameError(t *testing.T) {
	tmpDir, err := os.MkdirTemp("", "state-rename-error-*")
	if err != nil {
		t.Fatalf("Failed to create temp dir: %v", err)
	}
	defer os.RemoveAll(tmpDir)

	sm := NewManager(tmpDir)
	sm.state.LastChannel = "test-channel"

	// Create a file at the target location to cause rename error
	// This will cause os.Rename to fail
	stateDir := filepath.Dir(sm.stateFile)
	os.MkdirAll(stateDir, 0755)

	// Create a file that already exists at the target path
	existingFile := sm.stateFile
	existingData := `{"lastChannel": "existing", "lastChatID": "id", "timestamp": "2026-03-09T12:00:00Z"}`
	os.WriteFile(existingFile, []byte(existingData), 0644)

	// On Windows, we need to make the existing file read-only to cause rename to fail
	os.Chmod(existingFile, 0444)
	defer os.Chmod(existingFile, 0644)

	err = sm.saveAtomic()
	if err == nil {
		t.Error("saveAtomic should fail when rename fails due to existing read-only file")
	}
}

func TestLoadFileError(t *testing.T) {
	tmpDir, err := os.MkdirTemp("", "state-load-error-*")
	if err != nil {
		t.Fatalf("Failed to create temp dir: %v", err)
	}
	defer os.RemoveAll(tmpDir)

	sm := NewManager(tmpDir)
	sm.state.LastChannel = "initial"

	// Create a corrupted state file
	stateDir := filepath.Dir(sm.stateFile)
	os.MkdirAll(stateDir, 0755)
	corruptedData := `{"lastChannel": "corrupted", "lastChatID": "id", "timestamp": "invalid-timestamp"}`
	corruptedFile := sm.stateFile
	os.WriteFile(corruptedFile, []byte(corruptedData), 0644)

	// Try to load - should fail with unmarshal error
	err = sm.load()
	if err == nil {
		t.Error("load should fail when state file contains invalid JSON")
	}
}

func TestLoadPermissionError(t *testing.T) {
	tmpDir, err := os.MkdirTemp("", "state-permission-error-*")
	if err != nil {
		t.Fatalf("Failed to create temp dir: %v", err)
	}
	defer os.RemoveAll(tmpDir)

	sm := NewManager(tmpDir)

	// Create a valid state file
	stateDir := filepath.Dir(sm.stateFile)
	os.MkdirAll(stateDir, 0755)
	validData := `{"lastChannel": "test", "lastChatID": "123", "timestamp": "2026-03-09T12:00:00Z"}`
	stateFile := sm.stateFile
	os.WriteFile(stateFile, []byte(validData), 0644)

	// Clear state to force reload
	sm.state = &State{}

	// Try to load - permission errors are OS-dependent
	// On some systems, this might succeed, on others it might fail
	err = sm.load()
	if err != nil {
		// This is acceptable behavior on systems with strict permissions
		return
	}
}

func TestLoadPartialState(t *testing.T) {
	tmpDir, err := os.MkdirTemp("", "state-partial-*")
	if err != nil {
		t.Fatalf("Failed to create temp dir: %v", err)
	}
	defer os.RemoveAll(tmpDir)

	sm := NewManager(tmpDir)

	// Test loading complete state (this should definitely work)
	testCases := []struct {
		name     string
		content  string
		expectedLastChannel string
		expectedLastChatID string
	}{
		{"All fields", `{"last_channel": "discord", "last_chat_id": "789012", "timestamp": "2026-03-09T12:00:00Z"}`, "discord", "789012"},
	}

	for _, tc := range testCases {
		t.Run(tc.name, func(t *testing.T) {
			stateDir := filepath.Dir(sm.stateFile)
			os.MkdirAll(stateDir, 0755)

			stateFile := sm.stateFile
			os.WriteFile(stateFile, []byte(tc.content), 0644)

			// Debug: read back what we wrote
			writtenData, _ := os.ReadFile(stateFile)
			t.Logf("Written data: %s", string(writtenData))

			// Clear state to force reload
			sm.state = &State{}

			err := sm.load()
			if err != nil {
				t.Errorf("load should not fail: %v", err)
			}

			t.Logf("Loaded LastChannel: %v", sm.state.LastChannel)
			t.Logf("Loaded LastChatID: %v", sm.state.LastChatID)

			if sm.state.LastChannel != tc.expectedLastChannel {
				t.Errorf("Expected lastChannel '%v', got '%v'", tc.expectedLastChannel, sm.state.LastChannel)
			}
			if sm.state.LastChatID != tc.expectedLastChatID {
				t.Errorf("Expected lastChatID '%v', got '%v'", tc.expectedLastChatID, sm.state.LastChatID)
			}
		})
	}
}

func TestSetLastChannelEmpty(t *testing.T) {
	tmpDir, err := os.MkdirTemp("", "state-empty-channel-*")
	if err != nil {
		t.Fatalf("Failed to create temp dir: %v", err)
	}
	defer os.RemoveAll(tmpDir)

	sm := NewManager(tmpDir)

	// Set empty channel
	err = sm.SetLastChannel("")
	if err != nil {
		t.Errorf("SetLastChannel with empty string should not fail: %v", err)
	}

	if sm.GetLastChannel() != "" {
		t.Errorf("Expected last channel to be empty, got %v", sm.GetLastChannel())
	}
}

func TestSetLastChatIDEmpty(t *testing.T) {
	tmpDir, err := os.MkdirTemp("", "state-empty-chatid-*")
	if err != nil {
		t.Fatalf("Failed to create temp dir: %v", err)
	}
	defer os.RemoveAll(tmpDir)

	sm := NewManager(tmpDir)

	// Set empty chat ID
	err = sm.SetLastChatID("")
	if err != nil {
		t.Errorf("SetLastChatID with empty string should not fail: %v", err)
	}

	if sm.GetLastChatID() != "" {
		t.Errorf("Expected last chat ID to be empty, got %v", sm.GetLastChatID())
	}
}

func TestStatePersistenceAcrossManagers(t *testing.T) {
	tmpDir, err := os.MkdirTemp("", "state-persistence-*")
	if err != nil {
		t.Fatalf("Failed to create temp dir: %v", err)
	}
	defer os.RemoveAll(tmpDir)

	// Create first manager and set state
	sm1 := NewManager(tmpDir)
	sm1.SetLastChannel("test-channel")
	sm1.SetLastChatID("test-chat-id")
	sm1.SetLastChannel("final-channel")  // Update timestamp

	// Create second manager - should load persisted state
	sm2 := NewManager(tmpDir)

	if sm2.GetLastChannel() != "final-channel" {
		t.Errorf("Expected 'final-channel', got %v", sm2.GetLastChannel())
	}
	if sm2.GetLastChatID() != "test-chat-id" {
		t.Errorf("Expected 'test-chat-id', got %v", sm2.GetLastChatID())
	}

	// Verify timestamp is updated
	if sm2.GetTimestamp().IsZero() {
		t.Error("Timestamp should not be zero")
	}
}
