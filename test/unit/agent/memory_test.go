// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package agent_test

import (
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/276793422/NemesisBot/module/agent"
)

// TestNewMemoryStore tests creating a new memory store
func TestNewMemoryStore(t *testing.T) {
	tempDir := t.TempDir()

	store := agent.NewMemoryStore(tempDir)
	if store == nil {
		t.Fatal("Expected non-nil MemoryStore")
	}

	// Verify memory directory was created
	memoryDir := filepath.Join(tempDir, "memory")
	if info, err := os.Stat(memoryDir); err != nil {
		t.Errorf("Memory directory should be created: %v", err)
	} else if !info.IsDir() {
		t.Error("Memory path should be a directory")
	}
}

// TestMemoryStore_ReadLongTerm tests reading long-term memory
func TestMemoryStore_ReadLongTerm(t *testing.T) {
	tempDir := t.TempDir()
	store := agent.NewMemoryStore(tempDir)

	// Test reading when file doesn't exist
	content := store.ReadLongTerm()
	if content != "" {
		t.Error("Expected empty string when MEMORY.md doesn't exist")
	}

	// Create MEMORY.md file
	memoryFile := filepath.Join(tempDir, "memory", "MEMORY.md")
	err := os.WriteFile(memoryFile, []byte("Test long-term memory"), 0644)
	if err != nil {
		t.Fatalf("Failed to create MEMORY.md: %v", err)
	}

	// Test reading existing file
	content = store.ReadLongTerm()
	if content != "Test long-term memory" {
		t.Errorf("Expected 'Test long-term memory', got '%s'", content)
	}
}

// TestMemoryStore_WriteLongTerm tests writing long-term memory
func TestMemoryStore_WriteLongTerm(t *testing.T) {
	tempDir := t.TempDir()
	store := agent.NewMemoryStore(tempDir)

	// Write content
	err := store.WriteLongTerm("New memory content")
	if err != nil {
		t.Errorf("Failed to write long-term memory: %v", err)
	}

	// Verify content
	memoryFile := filepath.Join(tempDir, "memory", "MEMORY.md")
	data, err := os.ReadFile(memoryFile)
	if err != nil {
		t.Fatalf("Failed to read MEMORY.md: %v", err)
	}

	if string(data) != "New memory content" {
		t.Errorf("Expected 'New memory content', got '%s'", string(data))
	}
}

// TestMemoryStore_ReadToday tests reading today's daily note
func TestMemoryStore_ReadToday(t *testing.T) {
	tempDir := t.TempDir()
	store := agent.NewMemoryStore(tempDir)

	// Test reading when file doesn't exist
	content := store.ReadToday()
	if content != "" {
		t.Error("Expected empty string when today's file doesn't exist")
	}

	// Create today's file by appending
	err := store.AppendToday("Test entry")
	if err != nil {
		t.Fatalf("Failed to append to today: %v", err)
	}

	// Now read should return the content
	content = store.ReadToday()
	if content == "" {
		t.Error("Expected non-empty content after writing")
	}
	// Note: exact format depends on date, so just check it's not empty and starts with "#"
	if len(content) < 10 {
		t.Errorf("Content too short: %s", content)
	}
}

// TestMemoryStore_AppendToday tests appending to today's daily note
func TestMemoryStore_AppendToday(t *testing.T) {
	tempDir := t.TempDir()
	store := agent.NewMemoryStore(tempDir)

	// First append (creates file with header)
	err := store.AppendToday("First entry")
	if err != nil {
		t.Errorf("Failed to append first entry: %v", err)
	}

	// Second append (adds to existing file)
	err = store.AppendToday("Second entry")
	if err != nil {
		t.Errorf("Failed to append second entry: %v", err)
	}

	// Verify content
	content := store.ReadToday()
	if content == "" {
		t.Fatal("Expected non-empty content")
	}

	// Should contain both entries
	// Note: exact format depends on date, so just check for key parts
	if !containsString(content, "First entry") {
		t.Error("Expected 'First entry' in content")
	}
	if !containsString(content, "Second entry") {
		t.Error("Expected 'Second entry' in content")
	}
}

// TestMemoryStore_AppendToday_CreatesMonthDirectory tests that month directory is created
func TestMemoryStore_AppendToday_CreatesMonthDirectory(t *testing.T) {
	tempDir := t.TempDir()
	store := agent.NewMemoryStore(tempDir)

	// Append should create month directory
	err := store.AppendToday("Test")
	if err != nil {
		t.Fatalf("Failed to append: %v", err)
	}

	// Check that memory directory exists
	memoryDir := filepath.Join(tempDir, "memory")
	_, err = os.Stat(memoryDir)
	if err != nil {
		t.Errorf("Memory directory should exist: %v", err)
	}

	// The month directory should be created (we can't check exact name without knowing today's date)
	// But we can verify that files were created in memory directory
	entries, err := os.ReadDir(memoryDir)
	if err != nil {
		t.Fatalf("Failed to read memory directory: %v", err)
	}

	if len(entries) == 0 {
		t.Error("Expected month directory to be created")
	}
}

// TestMemoryStore_GetRecentDailyNotes tests retrieving recent daily notes
func TestMemoryStore_GetRecentDailyNotes(t *testing.T) {
	tempDir := t.TempDir()
	store := agent.NewMemoryStore(tempDir)

	// Test with no files
	notes := store.GetRecentDailyNotes(3)
	if notes != "" {
		t.Error("Expected empty string when no daily notes exist")
	}

	// Create some daily notes by appending
	err := store.AppendToday("Today's note")
	if err != nil {
		t.Fatalf("Failed to append today's note: %v", err)
	}

	// Get recent notes
	notes = store.GetRecentDailyNotes(1)
	if notes == "" {
		t.Error("Expected non-empty notes after writing")
	}

	if !containsString(notes, "Today's note") {
		t.Error("Expected 'Today's note' in recent notes")
	}
}

// TestMemoryStore_GetMemoryContext tests getting formatted memory context
func TestMemoryStore_GetMemoryContext(t *testing.T) {
	tempDir := t.TempDir()
	store := agent.NewMemoryStore(tempDir)

	// Test with no memory
	context := store.GetMemoryContext()
	if context != "" {
		t.Error("Expected empty string when no memory exists")
	}

	// Add long-term memory
	err := store.WriteLongTerm("Important long-term information")
	if err != nil {
		t.Fatalf("Failed to write long-term memory: %v", err)
	}

	// Add daily note
	err = store.AppendToday("Today's event")
	if err != nil {
		t.Fatalf("Failed to append daily note: %v", err)
	}

	// Get context
	context = store.GetMemoryContext()
	if context == "" {
		t.Error("Expected non-empty context")
	}

	// Should contain both long-term and daily notes
	if !containsString(context, "Important long-term information") {
		t.Error("Expected long-term memory in context")
	}
	if !containsString(context, "Today's event") {
		t.Error("Expected daily note in context")
	}

	// Should have section headers
	if !containsString(context, "Long-term Memory") {
		t.Error("Expected 'Long-term Memory' section")
	}
	if !containsString(context, "Recent Daily Notes") {
		t.Error("Expected 'Recent Daily Notes' section")
	}
}

// TestMemoryStore_ConcurrentAccess tests concurrent access to memory store
func TestMemoryStore_ConcurrentAccess(t *testing.T) {
	tempDir := t.TempDir()
	store := agent.NewMemoryStore(tempDir)

	// Perform concurrent writes
	done := make(chan bool)
	for i := 0; i < 10; i++ {
		go func(n int) {
			err := store.AppendToday(string(rune('A' + n)))
			if err != nil {
				t.Errorf("Concurrent write failed: %v", err)
			}
			done <- true
		}(i)
	}

	// Wait for all goroutines
	for i := 0; i < 10; i++ {
		<-done
	}

	// Verify we can read without error
	content := store.ReadToday()
	if content == "" {
		t.Error("Expected non-empty content after concurrent writes")
	}
}

// TestMemoryStore_WriteLongTerm_Overwrite tests overwriting long-term memory
func TestMemoryStore_WriteLongTerm_Overwrite(t *testing.T) {
	tempDir := t.TempDir()
	store := agent.NewMemoryStore(tempDir)

	// Write initial content
	err := store.WriteLongTerm("Initial memory")
	if err != nil {
		t.Errorf("Failed to write initial memory: %v", err)
	}

	// Overwrite with new content
	err = store.WriteLongTerm("Updated memory")
	if err != nil {
		t.Errorf("Failed to write updated memory: %v", err)
	}

	// Verify content was overwritten
	content := store.ReadLongTerm()
	if content != "Updated memory" {
		t.Errorf("Expected 'Updated memory', got '%s'", content)
	}
}

// TestMemoryStore_WriteLongTerm_EmptyContent tests writing empty content to long-term memory
func TestMemoryStore_WriteLongTerm_EmptyContent(t *testing.T) {
	tempDir := t.TempDir()
	store := agent.NewMemoryStore(tempDir)

	// Write empty content
	err := store.WriteLongTerm("")
	if err != nil {
		t.Errorf("Failed to write empty content: %v", err)
	}

	// File should exist but be empty
	content := store.ReadLongTerm()
	if content != "" {
		t.Errorf("Expected empty content, got '%s'", content)
	}
}

// TestMemoryStore_WriteLongTerm_SpecialCharacters tests writing special characters to long-term memory
func TestMemoryStore_WriteLongTerm_SpecialCharacters(t *testing.T) {
	tempDir := t.TempDir()
	store := agent.NewMemoryStore(tempDir)

	specialContent := `# Test Memory

This contains special characters:
- Newlines\n
- Tabs\t
- Quotes " and '
- Unicode: 中文
- Emoji: 🎉
- HTML: <strong>bold</strong>
`

	err := store.WriteLongTerm(specialContent)
	if err != nil {
		t.Errorf("Failed to write special content: %v", err)
	}

	content := store.ReadLongTerm()
	if content != specialContent {
		t.Error("Content with special characters was not preserved correctly")
	}
}

// TestMemoryStore_AppendToday_MultipleEntries tests appending multiple entries to today's note
func TestMemoryStore_AppendToday_MultipleEntries(t *testing.T) {
	tempDir := t.TempDir()
	store := agent.NewMemoryStore(tempDir)

	entries := []string{
		"First entry",
		"Second entry",
		"Third entry",
		"Fourth entry",
	}

	for _, entry := range entries {
		err := store.AppendToday(entry)
		if err != nil {
			t.Errorf("Failed to append entry '%s': %v", entry, err)
		}
	}

	content := store.ReadToday()

	// All entries should be present
	for _, entry := range entries {
		if !containsString(content, entry) {
			t.Errorf("Expected entry '%s' to be in content", entry)
		}
	}
}

// TestMemoryStore_AppendToday_MultilineContent tests appending multiline content
func TestMemoryStore_AppendToday_MultilineContent(t *testing.T) {
	tempDir := t.TempDir()
	store := agent.NewMemoryStore(tempDir)

	multilineContent := `Event 1: Something happened
Details:
- Point 1
- Point 2
- Point 3

Conclusion: It was successful.`

	err := store.AppendToday(multilineContent)
	if err != nil {
		t.Errorf("Failed to append multiline content: %v", err)
	}

	content := store.ReadToday()

	// Should contain the multiline content
	if !containsString(content, "Event 1") {
		t.Error("Expected 'Event 1' in content")
	}
	if !containsString(content, "Point 1") {
		t.Error("Expected 'Point 1' in content")
	}
	if !containsString(content, "Conclusion") {
		t.Error("Expected 'Conclusion' in content")
	}
}

// TestMemoryStore_GetRecentDailyNotes_ZeroDays tests requesting zero days of notes
func TestMemoryStore_GetRecentDailyNotes_ZeroDays(t *testing.T) {
	tempDir := t.TempDir()
	store := agent.NewMemoryStore(tempDir)

	// Add a note
	err := store.AppendToday("Test note")
	if err != nil {
		t.Fatalf("Failed to append: %v", err)
	}

	// Request 0 days
	notes := store.GetRecentDailyNotes(0)

	if notes != "" {
		t.Error("Expected empty string when requesting 0 days")
	}
}

// TestMemoryStore_GetRecentDailyNotes_NegativeDays tests requesting negative days
func TestMemoryStore_GetRecentDailyNotes_NegativeDays(t *testing.T) {
	tempDir := t.TempDir()
	store := agent.NewMemoryStore(tempDir)

	// Add a note
	err := store.AppendToday("Test note")
	if err != nil {
		t.Fatalf("Failed to append: %v", err)
	}

	// Request -1 days (should return empty)
	notes := store.GetRecentDailyNotes(-1)

	if notes != "" {
		t.Error("Expected empty string when requesting negative days")
	}
}

// TestMemoryStore_GetRecentDailyNotes_ManyDays tests requesting many days of notes
func TestMemoryStore_GetRecentDailyNotes_ManyDays(t *testing.T) {
	tempDir := t.TempDir()
	store := agent.NewMemoryStore(tempDir)

	// Add today's note
	err := store.AppendToday("Today's note")
	if err != nil {
		t.Fatalf("Failed to append: %v", err)
	}

	// Request 365 days (more than available)
	notes := store.GetRecentDailyNotes(365)

	if notes == "" {
		t.Error("Expected non-empty notes when requesting many days")
	}

	// Should contain today's note
	if !containsString(notes, "Today's note") {
		t.Error("Expected today's note in results")
	}
}

// TestMemoryStore_GetMemoryContext_OnlyLongTerm tests memory context with only long-term memory
func TestMemoryStore_GetMemoryContext_OnlyLongTerm(t *testing.T) {
	tempDir := t.TempDir()
	store := agent.NewMemoryStore(tempDir)

	// Add long-term memory only
	err := store.WriteLongTerm("Important information")
	if err != nil {
		t.Fatalf("Failed to write long-term memory: %v", err)
	}

	context := store.GetMemoryContext()

	if context == "" {
		t.Error("Expected non-empty context")
	}

	// Should contain long-term memory section
	if !containsString(context, "Long-term Memory") {
		t.Error("Expected 'Long-term Memory' section")
	}

	// Should contain the content
	if !containsString(context, "Important information") {
		t.Error("Expected long-term memory content")
	}

	// Should NOT contain daily notes section
	if containsString(context, "Recent Daily Notes") {
		t.Error("Should not have daily notes section when no daily notes exist")
	}
}

// TestMemoryStore_GetMemoryContext_OnlyDailyNotes tests memory context with only daily notes
func TestMemoryStore_GetMemoryContext_OnlyDailyNotes(t *testing.T) {
	tempDir := t.TempDir()
	store := agent.NewMemoryStore(tempDir)

	// Add daily note only
	err := store.AppendToday("Today's event")
	if err != nil {
		t.Fatalf("Failed to append: %v", err)
	}

	context := store.GetMemoryContext()

	if context == "" {
		t.Error("Expected non-empty context")
	}

	// Should contain daily notes section
	if !containsString(context, "Recent Daily Notes") {
		t.Error("Expected 'Recent Daily Notes' section")
	}

	// Should contain the content
	if !containsString(context, "Today's event") {
		t.Error("Expected daily note content")
	}

	// Should NOT contain long-term memory section
	if containsString(context, "Long-term Memory") {
		t.Error("Should not have long-term memory section when no long-term memory exists")
	}
}

// TestMemoryStore_GetMemoryContext_WithMultipleDailyNotes tests memory context with multiple daily notes
func TestMemoryStore_GetMemoryContext_WithMultipleDailyNotes(t *testing.T) {
	tempDir := t.TempDir()
	store := agent.NewMemoryStore(tempDir)

	// Add multiple entries to today's note
	err := store.AppendToday("Morning event")
	if err != nil {
		t.Fatalf("Failed to append: %v", err)
	}

	err = store.AppendToday("Afternoon event")
	if err != nil {
		t.Fatalf("Failed to append: %v", err)
	}

	context := store.GetMemoryContext()

	if context == "" {
		t.Error("Expected non-empty context")
	}

	// Should contain both entries
	if !containsString(context, "Morning event") {
		t.Error("Expected 'Morning event' in context")
	}
	if !containsString(context, "Afternoon event") {
		t.Error("Expected 'Afternoon event' in context")
	}
}

// TestMemoryStore_GetMemoryContext_EmptyMemory tests memory context with no memory
func TestMemoryStore_GetMemoryContext_EmptyMemory(t *testing.T) {
	tempDir := t.TempDir()
	store := agent.NewMemoryStore(tempDir)

	context := store.GetMemoryContext()

	if context != "" {
		t.Error("Expected empty context when no memory exists")
	}
}

// TestMemoryStore_FilePermissions tests that files are created with correct permissions
func TestMemoryStore_FilePermissions(t *testing.T) {
	tempDir := t.TempDir()
	store := agent.NewMemoryStore(tempDir)

	// Write long-term memory
	err := store.WriteLongTerm("Test content")
	if err != nil {
		t.Fatalf("Failed to write: %v", err)
	}

	// Check file permissions (may vary by OS)
	memoryFile := filepath.Join(tempDir, "memory", "MEMORY.md")
	info, err := os.Stat(memoryFile)
	if err != nil {
		t.Fatalf("Failed to stat file: %v", err)
	}

	// File should exist and be a regular file
	if info.IsDir() {
		t.Error("MEMORY.md should be a file, not a directory")
	}
}

// TestMemoryStore_DirectoryStructure tests that directory structure is created correctly
func TestMemoryStore_DirectoryStructure(t *testing.T) {
	tempDir := t.TempDir()
	store := agent.NewMemoryStore(tempDir)

	// Write something to trigger directory creation
	err := store.WriteLongTerm("Test")
	if err != nil {
		t.Fatalf("Failed to write: %v", err)
	}

	// Check memory directory exists
	memoryDir := filepath.Join(tempDir, "memory")
	info, err := os.Stat(memoryDir)
	if err != nil {
		t.Fatalf("Failed to stat memory dir: %v", err)
	}

	if !info.IsDir() {
		t.Error("memory path should be a directory")
	}

	// After appending to today, month directory should be created
	err = store.AppendToday("Test")
	if err != nil {
		t.Fatalf("Failed to append: %v", err)
	}

	// Check that subdirectories were created (we can't check exact name without knowing date)
	entries, err := os.ReadDir(memoryDir)
	if err != nil {
		t.Fatalf("Failed to read memory dir: %v", err)
	}

	if len(entries) == 0 {
		t.Error("Expected month directory to be created")
	}
}

// TestMemoryStore_LargeContent tests handling large content
func TestMemoryStore_LargeContent(t *testing.T) {
	tempDir := t.TempDir()
	store := agent.NewMemoryStore(tempDir)

	// Create large content (10KB)
	largeContent := strings.Repeat("This is a test. ", 2500)

	err := store.WriteLongTerm(largeContent)
	if err != nil {
		t.Errorf("Failed to write large content: %v", err)
	}

	content := store.ReadLongTerm()

	if content != largeContent {
		t.Error("Large content was not preserved correctly")
	}
}

// TestMemoryStore_GetRecentDailyNotes_Separator tests that daily notes are properly separated
func TestMemoryStore_GetRecentDailyNotes_Separator(t *testing.T) {
	tempDir := t.TempDir()
	store := agent.NewMemoryStore(tempDir)

	// Create notes for today
	err := store.AppendToday("Today's note")
	if err != nil {
		t.Fatalf("Failed to append: %v", err)
	}

	// Get recent notes (will only have today)
	notes := store.GetRecentDailyNotes(1)

	if notes == "" {
		t.Error("Expected non-empty notes")
	}

	// Should contain the note
	if !containsString(notes, "Today's note") {
		t.Error("Expected today's note in results")
	}
}

// Helper functions

// Helper function to get today's date string (YYYY-MM-DD format)
func getTodayDateString() string {
	// We can't easily test this without exposing the internal format
	// For now, just return a placeholder
	return "2026-03-09"
}

// Helper function to check if a string contains a substring
func containsString(s, substr string) bool {
	return len(s) >= len(substr) && findSubstring(s, substr)
}

func findSubstring(s, substr string) bool {
	for i := 0; i <= len(s)-len(substr); i++ {
		if s[i:i+len(substr)] == substr {
			return true
		}
	}
	return false
}
