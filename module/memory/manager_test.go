package memory_test

import (
	"context"
	"strings"
	"testing"
	"time"

	"github.com/276793422/NemesisBot/module/memory"
)

// ---------------------------------------------------------------------------
// MemoryType constants and String()
// ---------------------------------------------------------------------------

func TestMemoryTypeString(t *testing.T) {
	tests := []struct {
		mt       memory.MemoryType
		expected string
	}{
		{memory.MemoryShortTerm, "short_term"},
		{memory.MemoryLongTerm, "long_term"},
		{memory.MemoryEpisodic, "episodic"},
		{memory.MemoryGraph, "graph"},
		{memory.MemoryDaily, "daily"},
		{memory.MemoryType(99), "unknown"},
	}
	for _, tt := range tests {
		got := tt.mt.String()
		if got != tt.expected {
			t.Errorf("MemoryType(%d).String() = %q, want %q", tt.mt, got, tt.expected)
		}
	}
}

// ---------------------------------------------------------------------------
// ParseMemoryType
// ---------------------------------------------------------------------------

func TestParseMemoryType(t *testing.T) {
	tests := []struct {
		input    string
		expected memory.MemoryType
	}{
		{"short_term", memory.MemoryShortTerm},
		{"long_term", memory.MemoryLongTerm},
		{"episodic", memory.MemoryEpisodic},
		{"graph", memory.MemoryGraph},
		{"daily", memory.MemoryDaily},
		{"unknown", memory.MemoryLongTerm}, // default fallback
		{"", memory.MemoryLongTerm},        // default fallback
		{"INVALID", memory.MemoryLongTerm}, // default fallback
	}
	for _, tt := range tests {
		got := memory.ParseMemoryType(tt.input)
		if got != tt.expected {
			t.Errorf("ParseMemoryType(%q) = %d, want %d", tt.input, got, tt.expected)
		}
	}
}

// ---------------------------------------------------------------------------
// Entry struct creation and field access
// ---------------------------------------------------------------------------

func TestEntryCreation(t *testing.T) {
	now := time.Now().UTC()
	entry := &memory.Entry{
		ID:      "test-id-001",
		Type:    memory.MemoryLongTerm,
		Content: "test content",
		Metadata: map[string]string{
			"source": "unit-test",
		},
		Tags:      []string{"test", "example"},
		Score:     0.95,
		CreatedAt: now,
		UpdatedAt: now,
	}

	if entry.ID != "test-id-001" {
		t.Errorf("Entry.ID = %q, want %q", entry.ID, "test-id-001")
	}
	if entry.Type != memory.MemoryLongTerm {
		t.Errorf("Entry.Type = %d, want %d", entry.Type, memory.MemoryLongTerm)
	}
	if entry.Content != "test content" {
		t.Errorf("Entry.Content = %q, want %q", entry.Content, "test content")
	}
	if entry.Metadata["source"] != "unit-test" {
		t.Errorf("Entry.Metadata[source] = %q, want %q", entry.Metadata["source"], "unit-test")
	}
	if len(entry.Tags) != 2 {
		t.Errorf("len(Entry.Tags) = %d, want 2", len(entry.Tags))
	}
	if entry.Score != 0.95 {
		t.Errorf("Entry.Score = %f, want 0.95", entry.Score)
	}
	if !entry.CreatedAt.Equal(now) {
		t.Errorf("Entry.CreatedAt = %v, want %v", entry.CreatedAt, now)
	}
}

// ---------------------------------------------------------------------------
// SearchResult
// ---------------------------------------------------------------------------

func TestSearchResult(t *testing.T) {
	sr := &memory.SearchResult{
		Entries: []memory.Entry{
			{ID: "1", Content: "alpha"},
			{ID: "2", Content: "beta"},
		},
		Total: 2,
		Query: "test",
	}
	if sr.Total != 2 {
		t.Errorf("SearchResult.Total = %d, want 2", sr.Total)
	}
	if sr.Query != "test" {
		t.Errorf("SearchResult.Query = %q, want %q", sr.Query, "test")
	}
	if len(sr.Entries) != 2 {
		t.Errorf("len(SearchResult.Entries) = %d, want 2", len(sr.Entries))
	}
}

// ---------------------------------------------------------------------------
// Config and DefaultConfig
// ---------------------------------------------------------------------------

func TestDefaultConfig(t *testing.T) {
	cfg := memory.DefaultConfig()
	if cfg.Vector.Enabled {
		t.Error("DefaultConfig Vector.Enabled should be false")
	}
	if cfg.Vector.Backend != "local" {
		t.Errorf("DefaultConfig Vector.Backend = %q, want %q", cfg.Vector.Backend, "local")
	}
	if cfg.Vector.EmbeddingModel != "local" {
		t.Errorf("DefaultConfig Vector.EmbeddingModel = %q, want %q", cfg.Vector.EmbeddingModel, "local")
	}
	if cfg.Vector.MaxResults != 5 {
		t.Errorf("DefaultConfig Vector.MaxResults = %d, want 5", cfg.Vector.MaxResults)
	}
	if cfg.Vector.SimilarityThreshold != 0.7 {
		t.Errorf("DefaultConfig Vector.SimilarityThreshold = %f, want 0.7", cfg.Vector.SimilarityThreshold)
	}
	if cfg.Vector.RetentionDays != 90 {
		t.Errorf("DefaultConfig Vector.RetentionDays = %d, want 90", cfg.Vector.RetentionDays)
	}
	if cfg.Vector.StoragePath != "" {
		t.Errorf("DefaultConfig Vector.StoragePath = %q, want empty", cfg.Vector.StoragePath)
	}
}

// ---------------------------------------------------------------------------
// Manager creation
// ---------------------------------------------------------------------------

func TestNewManager_NilConfig(t *testing.T) {
	m, err := memory.NewManager(nil, t.TempDir())
	if err != nil {
		t.Fatalf("NewManager with nil config: %v", err)
	}
	if !m.IsEnabled() {
		t.Error("Manager should be enabled after creation")
	}
	if err := m.Close(); err != nil {
		t.Fatalf("Close: %v", err)
	}
}

func TestNewManager_ValidConfig(t *testing.T) {
	cfg := memory.DefaultConfig()
	m, err := memory.NewManager(cfg, t.TempDir())
	if err != nil {
		t.Fatalf("NewManager: %v", err)
	}
	defer m.Close()

	if !m.IsEnabled() {
		t.Error("Manager should be enabled")
	}
}

// ---------------------------------------------------------------------------
// Manager.Store and Query
// ---------------------------------------------------------------------------

func TestManager_StoreAndQuery(t *testing.T) {
	m, err := memory.NewManager(nil, t.TempDir())
	if err != nil {
		t.Fatalf("NewManager: %v", err)
	}
	defer m.Close()

	ctx := context.Background()

	entry := &memory.Entry{
		Type:    memory.MemoryLongTerm,
		Content: "The Go programming language is statically typed",
		Tags:    []string{"programming", "golang"},
	}
	if err := m.Store(ctx, entry); err != nil {
		t.Fatalf("Store: %v", err)
	}
	if entry.ID == "" {
		t.Error("Store should assign an ID to the entry")
	}
	if entry.CreatedAt.IsZero() {
		t.Error("Store should set CreatedAt")
	}
	if entry.UpdatedAt.IsZero() {
		t.Error("Store should set UpdatedAt")
	}

	// Query by keyword
	result, err := m.Query(ctx, "Go programming", 5, nil)
	if err != nil {
		t.Fatalf("Query: %v", err)
	}
	if result.Total == 0 {
		t.Error("Query should find at least one result")
	}
}

func TestManager_StoreWithTypeFilter(t *testing.T) {
	m, err := memory.NewManager(nil, t.TempDir())
	if err != nil {
		t.Fatalf("NewManager: %v", err)
	}
	defer m.Close()

	ctx := context.Background()

	// Store entries of different types
	_ = m.Store(ctx, &memory.Entry{Type: memory.MemoryLongTerm, Content: "long term fact"})
	_ = m.Store(ctx, &memory.Entry{Type: memory.MemoryEpisodic, Content: "episodic event"})

	// Query only long_term
	result, err := m.Query(ctx, "fact", 10, []memory.MemoryType{memory.MemoryLongTerm})
	if err != nil {
		t.Fatalf("Query with type filter: %v", err)
	}
	if result.Total == 0 {
		t.Error("Should find long_term entries")
	}

	// Query only episodic — should not match "fact"
	result2, err := m.Query(ctx, "fact", 10, []memory.MemoryType{memory.MemoryEpisodic})
	if err != nil {
		t.Fatalf("Query episodic: %v", err)
	}
	if result2.Total != 0 {
		t.Error("Should not find 'fact' in episodic entries")
	}
}

// ---------------------------------------------------------------------------
// Manager.Get and Delete
// ---------------------------------------------------------------------------

func TestManager_GetAndDelete(t *testing.T) {
	m, err := memory.NewManager(nil, t.TempDir())
	if err != nil {
		t.Fatalf("NewManager: %v", err)
	}
	defer m.Close()

	ctx := context.Background()

	entry := &memory.Entry{Type: memory.MemoryLongTerm, Content: "unique content for get test"}
	if err := m.Store(ctx, entry); err != nil {
		t.Fatalf("Store: %v", err)
	}

	// Get by ID
	got, err := m.Get(ctx, entry.ID)
	if err != nil {
		t.Fatalf("Get: %v", err)
	}
	if got == nil {
		t.Fatal("Get should return the entry")
	}
	if got.Content != "unique content for get test" {
		t.Errorf("Get Content = %q, want %q", got.Content, "unique content for get test")
	}

	// Delete
	if err := m.Delete(ctx, entry.ID); err != nil {
		t.Fatalf("Delete: %v", err)
	}

	got2, err := m.Get(ctx, entry.ID)
	if err != nil {
		t.Fatalf("Get after delete: %v", err)
	}
	if got2 != nil {
		t.Error("Get after delete should return nil")
	}
}

func TestManager_GetNonExistent(t *testing.T) {
	m, err := memory.NewManager(nil, t.TempDir())
	if err != nil {
		t.Fatalf("NewManager: %v", err)
	}
	defer m.Close()

	ctx := context.Background()
	got, err := m.Get(ctx, "nonexistent-id")
	if err != nil {
		t.Fatalf("Get non-existent: %v", err)
	}
	if got != nil {
		t.Error("Get non-existent should return nil")
	}
}

// ---------------------------------------------------------------------------
// Manager.List
// ---------------------------------------------------------------------------

func TestManager_List(t *testing.T) {
	m, err := memory.NewManager(nil, t.TempDir())
	if err != nil {
		t.Fatalf("NewManager: %v", err)
	}
	defer m.Close()

	ctx := context.Background()

	_ = m.Store(ctx, &memory.Entry{Type: memory.MemoryLongTerm, Content: "item A"})
	_ = m.Store(ctx, &memory.Entry{Type: memory.MemoryEpisodic, Content: "item B"})
	_ = m.Store(ctx, &memory.Entry{Type: memory.MemoryLongTerm, Content: "item C"})

	// List all
	result, err := m.List(ctx, nil, 0, 10)
	if err != nil {
		t.Fatalf("List all: %v", err)
	}
	if result.Total != 3 {
		t.Errorf("List all Total = %d, want 3", result.Total)
	}

	// List only long_term
	result2, err := m.List(ctx, []memory.MemoryType{memory.MemoryLongTerm}, 0, 10)
	if err != nil {
		t.Fatalf("List long_term: %v", err)
	}
	if result2.Total != 2 {
		t.Errorf("List long_term Total = %d, want 2", result2.Total)
	}
}

func TestManager_ListPagination(t *testing.T) {
	m, err := memory.NewManager(nil, t.TempDir())
	if err != nil {
		t.Fatalf("NewManager: %v", err)
	}
	defer m.Close()

	ctx := context.Background()
	for i := 0; i < 5; i++ {
		_ = m.Store(ctx, &memory.Entry{Type: memory.MemoryLongTerm, Content: "item"})
	}

	// offset beyond total → empty
	result, err := m.List(ctx, nil, 100, 10)
	if err != nil {
		t.Fatalf("List with large offset: %v", err)
	}
	if len(result.Entries) != 0 {
		t.Errorf("Entries with offset beyond total = %d, want 0", len(result.Entries))
	}

	// limit 2
	result2, err := m.List(ctx, nil, 0, 2)
	if err != nil {
		t.Fatalf("List with limit 2: %v", err)
	}
	if len(result2.Entries) != 2 {
		t.Errorf("Entries with limit 2 = %d, want 2", len(result2.Entries))
	}
	if result2.Total != 5 {
		t.Errorf("Total = %d, want 5", result2.Total)
	}
}

// ---------------------------------------------------------------------------
// Manager.QuerySemantic
// ---------------------------------------------------------------------------

func TestManager_QuerySemantic(t *testing.T) {
	m, err := memory.NewManager(nil, t.TempDir())
	if err != nil {
		t.Fatalf("NewManager: %v", err)
	}
	defer m.Close()

	ctx := context.Background()

	_ = m.Store(ctx, &memory.Entry{
		Type:    memory.MemoryLongTerm,
		Content: "Paris is the capital of France",
		Tags:    []string{"geography", "europe"},
	})
	_ = m.Store(ctx, &memory.Entry{
		Type:    memory.MemoryEpisodic,
		Content: "We discussed machine learning algorithms",
	})

	// Semantic query should use TF-IDF fallback
	result, err := m.QuerySemantic(ctx, "Paris France capital", 5)
	if err != nil {
		t.Fatalf("QuerySemantic: %v", err)
	}
	if result.Total == 0 {
		t.Error("QuerySemantic should find results")
	}
	if result.Query != "Paris France capital" {
		t.Errorf("QuerySemantic Query = %q, want %q", result.Query, "Paris France capital")
	}
}

func TestManager_QuerySemantic_DefaultLimit(t *testing.T) {
	m, err := memory.NewManager(nil, t.TempDir())
	if err != nil {
		t.Fatalf("NewManager: %v", err)
	}
	defer m.Close()

	ctx := context.Background()
	// With limit 0, should fall back to config default (5)
	result, err := m.QuerySemantic(ctx, "test", 0)
	if err != nil {
		t.Fatalf("QuerySemantic with limit 0: %v", err)
	}
	_ = result // just verify no error
}

// ---------------------------------------------------------------------------
// Manager.StoreEpisodic and session management
// ---------------------------------------------------------------------------

func TestManager_StoreEpisodic(t *testing.T) {
	m, err := memory.NewManager(nil, t.TempDir())
	if err != nil {
		t.Fatalf("NewManager: %v", err)
	}
	defer m.Close()

	ctx := context.Background()

	if err := m.StoreEpisodic(ctx, "session-123", "user", "Hello, how are you?"); err != nil {
		t.Fatalf("StoreEpisodic: %v", err)
	}

	// Verify stored as episodic type
	result, err := m.Query(ctx, "Hello", 10, []memory.MemoryType{memory.MemoryEpisodic})
	if err != nil {
		t.Fatalf("Query episodic: %v", err)
	}
	if result.Total == 0 {
		t.Error("Should find episodic entry")
	}

	// Verify metadata
	if len(result.Entries) > 0 {
		ep := result.Entries[0]
		if ep.Metadata["session_key"] != "session-123" {
			t.Errorf("session_key = %q, want %q", ep.Metadata["session_key"], "session-123")
		}
		if ep.Metadata["role"] != "user" {
			t.Errorf("role = %q, want %q", ep.Metadata["role"], "user")
		}
	}
}

// ---------------------------------------------------------------------------
// Manager.StoreFact
// ---------------------------------------------------------------------------

func TestManager_StoreFact(t *testing.T) {
	m, err := memory.NewManager(nil, t.TempDir())
	if err != nil {
		t.Fatalf("NewManager: %v", err)
	}
	defer m.Close()

	ctx := context.Background()

	tags := []string{"fact", "science"}
	if err := m.StoreFact(ctx, "Water boils at 100 degrees Celsius at sea level", tags); err != nil {
		t.Fatalf("StoreFact: %v", err)
	}

	result, err := m.Query(ctx, "water boils", 10, []memory.MemoryType{memory.MemoryLongTerm})
	if err != nil {
		t.Fatalf("Query long_term: %v", err)
	}
	if result.Total == 0 {
		t.Error("Should find the stored fact")
	}
}

// ---------------------------------------------------------------------------
// Manager.Close and cleanup
// ---------------------------------------------------------------------------

func TestManager_Close_DisablesManager(t *testing.T) {
	m, err := memory.NewManager(nil, t.TempDir())
	if err != nil {
		t.Fatalf("NewManager: %v", err)
	}

	if err := m.Close(); err != nil {
		t.Fatalf("Close: %v", err)
	}

	if m.IsEnabled() {
		t.Error("Manager should be disabled after Close")
	}
}

func TestManager_OperationsAfterClose(t *testing.T) {
	m, err := memory.NewManager(nil, t.TempDir())
	if err != nil {
		t.Fatalf("NewManager: %v", err)
	}
	m.Close()

	ctx := context.Background()

	// Store should be no-op
	entry := &memory.Entry{Type: memory.MemoryLongTerm, Content: "test"}
	if err := m.Store(ctx, entry); err != nil {
		t.Errorf("Store after close should be nil, got: %v", err)
	}

	// Query should return empty
	result, err := m.Query(ctx, "test", 10, nil)
	if err != nil {
		t.Errorf("Query after close error: %v", err)
	}
	if result.Total != 0 {
		t.Error("Query after close should return empty result")
	}

	// Get should return nil
	got, err := m.Get(ctx, "any-id")
	if err != nil {
		t.Errorf("Get after close error: %v", err)
	}
	if got != nil {
		t.Error("Get after close should return nil")
	}

	// Delete should be no-op
	if err := m.Delete(ctx, "any-id"); err != nil {
		t.Errorf("Delete after close error: %v", err)
	}

	// List should return empty
	listResult, err := m.List(ctx, nil, 0, 10)
	if err != nil {
		t.Errorf("List after close error: %v", err)
	}
	if len(listResult.Entries) != 0 {
		t.Error("List after close should return empty entries")
	}

	// QuerySemantic should return empty
	semResult, err := m.QuerySemantic(ctx, "test", 5)
	if err != nil {
		t.Errorf("QuerySemantic after close error: %v", err)
	}
	if semResult.Total != 0 {
		t.Error("QuerySemantic after close should return empty")
	}
}

// ---------------------------------------------------------------------------
// Edge cases
// ---------------------------------------------------------------------------

func TestManager_QueryEmpty(t *testing.T) {
	m, err := memory.NewManager(nil, t.TempDir())
	if err != nil {
		t.Fatalf("NewManager: %v", err)
	}
	defer m.Close()

	ctx := context.Background()

	result, err := m.Query(ctx, "", 10, nil)
	if err != nil {
		t.Fatalf("Query empty: %v", err)
	}
	if result.Total != 0 {
		t.Error("Empty query should return no results")
	}
}

func TestManager_StoreWithSpecialCharacters(t *testing.T) {
	m, err := memory.NewManager(nil, t.TempDir())
	if err != nil {
		t.Fatalf("NewManager: %v", err)
	}
	defer m.Close()

	ctx := context.Background()

	specialContent := "Special chars: <>&\"'\\n\\t\\x00 中文 日本語 한국어 🎉"
	entry := &memory.Entry{
		Type:    memory.MemoryLongTerm,
		Content: specialContent,
	}
	if err := m.Store(ctx, entry); err != nil {
		t.Fatalf("Store with special chars: %v", err)
	}

	got, err := m.Get(ctx, entry.ID)
	if err != nil {
		t.Fatalf("Get: %v", err)
	}
	if got == nil {
		t.Fatal("Get should return entry")
	}
	if got.Content != specialContent {
		t.Errorf("Content mismatch: got %q, want %q", got.Content, specialContent)
	}
}

func TestManager_StoreVeryLongContent(t *testing.T) {
	m, err := memory.NewManager(nil, t.TempDir())
	if err != nil {
		t.Fatalf("NewManager: %v", err)
	}
	defer m.Close()

	ctx := context.Background()

	longContent := strings.Repeat("abcdefghij ", 10000) // ~110KB
	entry := &memory.Entry{
		Type:    memory.MemoryLongTerm,
		Content: longContent,
	}
	if err := m.Store(ctx, entry); err != nil {
		t.Fatalf("Store long content: %v", err)
	}

	got, err := m.Get(ctx, entry.ID)
	if err != nil {
		t.Fatalf("Get: %v", err)
	}
	if got == nil {
		t.Fatal("Get should return entry")
	}
	if len(got.Content) != len(longContent) {
		t.Errorf("Content length mismatch: got %d, want %d", len(got.Content), len(longContent))
	}
}

func TestManager_StoreWithPresetID(t *testing.T) {
	m, err := memory.NewManager(nil, t.TempDir())
	if err != nil {
		t.Fatalf("NewManager: %v", err)
	}
	defer m.Close()

	ctx := context.Background()

	entry := &memory.Entry{
		ID:      "my-custom-id",
		Type:    memory.MemoryLongTerm,
		Content: "custom id test",
	}
	if err := m.Store(ctx, entry); err != nil {
		t.Fatalf("Store: %v", err)
	}
	if entry.ID != "my-custom-id" {
		t.Errorf("ID should be preserved, got %q", entry.ID)
	}
}

func TestManager_StoreWithPresetTimestamps(t *testing.T) {
	m, err := memory.NewManager(nil, t.TempDir())
	if err != nil {
		t.Fatalf("NewManager: %v", err)
	}
	defer m.Close()

	ctx := context.Background()

	presetTime := time.Date(2025, 1, 1, 0, 0, 0, 0, time.UTC)
	entry := &memory.Entry{
		Type:      memory.MemoryLongTerm,
		Content:   "timestamp test",
		CreatedAt: presetTime,
	}
	if err := m.Store(ctx, entry); err != nil {
		t.Fatalf("Store: %v", err)
	}
	// CreatedAt should be preserved since it was set
	if !entry.CreatedAt.Equal(presetTime) {
		t.Errorf("CreatedAt = %v, want %v", entry.CreatedAt, presetTime)
	}
	// UpdatedAt should be set to now
	if entry.UpdatedAt.IsZero() {
		t.Error("UpdatedAt should be set")
	}
}

func TestManager_StoreWithMetadataAndTags(t *testing.T) {
	m, err := memory.NewManager(nil, t.TempDir())
	if err != nil {
		t.Fatalf("NewManager: %v", err)
	}
	defer m.Close()

	ctx := context.Background()

	entry := &memory.Entry{
		Type:    memory.MemoryLongTerm,
		Content: "metadata search test",
		Metadata: map[string]string{
			"author": "Alice",
		},
		Tags: []string{"important"},
	}
	_ = m.Store(ctx, entry)

	// Query should match metadata
	result, err := m.Query(ctx, "Alice", 5, nil)
	if err != nil {
		t.Fatalf("Query: %v", err)
	}
	if result.Total == 0 {
		t.Error("Should find entry by metadata")
	}

	// Query should match tags
	result2, err := m.Query(ctx, "important", 5, nil)
	if err != nil {
		t.Fatalf("Query: %v", err)
	}
	if result2.Total == 0 {
		t.Error("Should find entry by tag")
	}
}

// ---------------------------------------------------------------------------
// Double close
// ---------------------------------------------------------------------------

func TestManager_DoubleClose(t *testing.T) {
	m, err := memory.NewManager(nil, t.TempDir())
	if err != nil {
		t.Fatalf("NewManager: %v", err)
	}
	if err := m.Close(); err != nil {
		t.Fatalf("First close: %v", err)
	}
	if err := m.Close(); err != nil {
		t.Fatalf("Second close: %v", err)
	}
}

// ---------------------------------------------------------------------------
// Concurrent access
// ---------------------------------------------------------------------------

func TestManager_ConcurrentStore(t *testing.T) {
	m, err := memory.NewManager(nil, t.TempDir())
	if err != nil {
		t.Fatalf("NewManager: %v", err)
	}
	defer m.Close()

	ctx := context.Background()
	done := make(chan bool, 10)

	for i := 0; i < 10; i++ {
		go func(idx int) {
			entry := &memory.Entry{
				Type:    memory.MemoryLongTerm,
				Content: "concurrent content",
			}
			_ = m.Store(ctx, entry)
			done <- true
		}(i)
	}

	for i := 0; i < 10; i++ {
		<-done
	}

	result, err := m.List(ctx, nil, 0, 100)
	if err != nil {
		t.Fatalf("List: %v", err)
	}
	if result.Total != 10 {
		t.Errorf("Expected 10 entries, got %d", result.Total)
	}
}
