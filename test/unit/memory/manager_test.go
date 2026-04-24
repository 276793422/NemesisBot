package memory_test

import (
	"context"
	"os"
	"path/filepath"
	"testing"

	"github.com/276793422/NemesisBot/module/memory"
)

// newTestManager creates a Manager backed by a temporary directory.
// The directory is cleaned up automatically when the test finishes.
func newTestManager(t *testing.T) *memory.Manager {
	t.Helper()
	tmp := t.TempDir()
	mgr, err := memory.NewManager(nil, tmp)
	if err != nil {
		t.Fatalf("NewManager: %v", err)
	}
	t.Cleanup(func() { mgr.Close() })
	return mgr
}

// --- Test cases ---

func TestManager_StoreAndGet(t *testing.T) {
	mgr := newTestManager(t)
	ctx := context.Background()

	entry := &memory.Entry{
		Type:    memory.MemoryLongTerm,
		Content: "The user prefers dark mode for all IDEs",
		Tags:    []string{"preference", "ide"},
	}

	if err := mgr.Store(ctx, entry); err != nil {
		t.Fatalf("Store: %v", err)
	}
	if entry.ID == "" {
		t.Fatal("expected entry.ID to be populated after Store")
	}

	got, err := mgr.Get(ctx, entry.ID)
	if err != nil {
		t.Fatalf("Get: %v", err)
	}
	if got == nil {
		t.Fatal("Get returned nil, expected the stored entry")
	}
	if got.Content != entry.Content {
		t.Errorf("Content mismatch: got %q, want %q", got.Content, entry.Content)
	}
	if got.Type != memory.MemoryLongTerm {
		t.Errorf("Type mismatch: got %v, want %v", got.Type, memory.MemoryLongTerm)
	}
	if got.CreatedAt.IsZero() {
		t.Error("expected CreatedAt to be set")
	}
}

func TestManager_Query(t *testing.T) {
	mgr := newTestManager(t)
	ctx := context.Background()

	// Store several entries with varying relevance.
	entries := []*memory.Entry{
		{Type: memory.MemoryLongTerm, Content: "The user prefers dark mode for IDEs"},
		{Type: memory.MemoryLongTerm, Content: "Server runs on port 8080"},
		{Type: memory.MemoryLongTerm, Content: "Dark theme reduces eye strain"},
		{Type: memory.MemoryLongTerm, Content: "Database connection string is set"},
	}
	for _, e := range entries {
		if err := mgr.Store(ctx, e); err != nil {
			t.Fatalf("Store: %v", err)
		}
	}

	// Query for "dark" -- should match entries about dark mode and dark theme.
	result, err := mgr.Query(ctx, "dark", 10, nil)
	if err != nil {
		t.Fatalf("Query: %v", err)
	}
	if result.Total < 2 {
		t.Errorf("expected at least 2 results for 'dark', got %d", result.Total)
	}

	// Results should be sorted by score descending.
	for i := 1; i < len(result.Entries); i++ {
		if result.Entries[i].Score > result.Entries[i-1].Score {
			t.Errorf("results not sorted by score: entry[%d].Score=%.4f > entry[%d].Score=%.4f",
				i, result.Entries[i].Score, i-1, result.Entries[i-1].Score)
		}
	}
}

func TestManager_QueryByType(t *testing.T) {
	mgr := newTestManager(t)
	ctx := context.Background()

	if err := mgr.Store(ctx, &memory.Entry{Type: memory.MemoryLongTerm, Content: "fact one"}); err != nil {
		t.Fatal(err)
	}
	if err := mgr.Store(ctx, &memory.Entry{Type: memory.MemoryEpisodic, Content: "episode one"}); err != nil {
		t.Fatal(err)
	}
	if err := mgr.Store(ctx, &memory.Entry{Type: memory.MemoryDaily, Content: "daily note"}); err != nil {
		t.Fatal(err)
	}

	result, err := mgr.Query(ctx, "one", 10, []memory.MemoryType{memory.MemoryEpisodic})
	if err != nil {
		t.Fatalf("Query: %v", err)
	}
	if result.Total != 1 {
		t.Errorf("expected 1 episodic result, got %d", result.Total)
	}
	if len(result.Entries) > 0 && result.Entries[0].Type != memory.MemoryEpisodic {
		t.Errorf("expected episodic type, got %v", result.Entries[0].Type)
	}
}

func TestManager_Delete(t *testing.T) {
	mgr := newTestManager(t)
	ctx := context.Background()

	entry := &memory.Entry{Type: memory.MemoryLongTerm, Content: "to be deleted"}
	if err := mgr.Store(ctx, entry); err != nil {
		t.Fatalf("Store: %v", err)
	}

	got, err := mgr.Get(ctx, entry.ID)
	if err != nil {
		t.Fatalf("Get before delete: %v", err)
	}
	if got == nil {
		t.Fatal("entry should exist before deletion")
	}

	if err := mgr.Delete(ctx, entry.ID); err != nil {
		t.Fatalf("Delete: %v", err)
	}

	got, err = mgr.Get(ctx, entry.ID)
	if err != nil {
		t.Fatalf("Get after delete: %v", err)
	}
	if got != nil {
		t.Error("expected nil after deletion")
	}
}

func TestManager_List(t *testing.T) {
	mgr := newTestManager(t)
	ctx := context.Background()

	for i := 0; i < 5; i++ {
		if err := mgr.Store(ctx, &memory.Entry{
			Type:    memory.MemoryLongTerm,
			Content: "entry",
		}); err != nil {
			t.Fatalf("Store %d: %v", i, err)
		}
	}

	// List first 3 entries.
	result, err := mgr.List(ctx, nil, 0, 3)
	if err != nil {
		t.Fatalf("List: %v", err)
	}
	if result.Total != 5 {
		t.Errorf("expected total=5, got %d", result.Total)
	}
	if len(result.Entries) != 3 {
		t.Errorf("expected 3 entries in page, got %d", len(result.Entries))
	}

	// List next page.
	result2, err := mgr.List(ctx, nil, 3, 3)
	if err != nil {
		t.Fatalf("List page2: %v", err)
	}
	if len(result2.Entries) != 2 {
		t.Errorf("expected 2 entries in second page, got %d", len(result2.Entries))
	}

	// List with type filter (should return 0 because entries are long_term).
	result3, err := mgr.List(ctx, []memory.MemoryType{memory.MemoryEpisodic}, 0, 10)
	if err != nil {
		t.Fatalf("List filtered: %v", err)
	}
	if result3.Total != 0 {
		t.Errorf("expected 0 episodic entries, got %d", result3.Total)
	}
}

func TestManager_StoreEpisodic(t *testing.T) {
	mgr := newTestManager(t)
	ctx := context.Background()

	if err := mgr.StoreEpisodic(ctx, "session-1", "user", "What is the weather?"); err != nil {
		t.Fatalf("StoreEpisodic: %v", err)
	}

	// Search for it via Query.
	result, err := mgr.Query(ctx, "weather", 10, []memory.MemoryType{memory.MemoryEpisodic})
	if err != nil {
		t.Fatalf("Query: %v", err)
	}
	if result.Total != 1 {
		t.Fatalf("expected 1 episodic result, got %d", result.Total)
	}

	ep := result.Entries[0]
	if ep.Type != memory.MemoryEpisodic {
		t.Errorf("expected type episodic, got %v", ep.Type)
	}
	if ep.Metadata["session_key"] != "session-1" {
		t.Errorf("expected session_key=session-1, got %q", ep.Metadata["session_key"])
	}
	if ep.Metadata["role"] != "user" {
		t.Errorf("expected role=user, got %q", ep.Metadata["role"])
	}
}

func TestManager_StoreFact(t *testing.T) {
	mgr := newTestManager(t)
	ctx := context.Background()

	if err := mgr.StoreFact(ctx, "Earth orbits the Sun", []string{"astronomy", "fact"}); err != nil {
		t.Fatalf("StoreFact: %v", err)
	}

	result, err := mgr.Query(ctx, "Earth Sun", 10, []memory.MemoryType{memory.MemoryLongTerm})
	if err != nil {
		t.Fatalf("Query: %v", err)
	}
	if result.Total != 1 {
		t.Fatalf("expected 1 result, got %d", result.Total)
	}
	if result.Entries[0].Type != memory.MemoryLongTerm {
		t.Errorf("expected long_term type, got %v", result.Entries[0].Type)
	}
}

func TestManager_QuerySemantic(t *testing.T) {
	mgr := newTestManager(t)
	ctx := context.Background()

	_ = mgr.Store(ctx, &memory.Entry{Type: memory.MemoryLongTerm, Content: "Go is a statically typed compiled language"})
	_ = mgr.Store(ctx, &memory.Entry{Type: memory.MemoryEpisodic, Content: "We discussed Go generics today"})
	_ = mgr.Store(ctx, &memory.Entry{Type: memory.MemoryDaily, Content: "Studied Go concurrency patterns"})

	result, err := mgr.QuerySemantic(ctx, "Go language", 5)
	if err != nil {
		t.Fatalf("QuerySemantic: %v", err)
	}
	if result.Total == 0 {
		t.Error("expected at least one result for 'Go language'")
	}
	// QuerySemantic searches across all types, so all 3 should potentially match.
	if result.Total < 1 {
		t.Errorf("expected at least 1 result, got %d", result.Total)
	}
}

func TestManager_Close(t *testing.T) {
	tmp := t.TempDir()
	mgr, err := memory.NewManager(nil, tmp)
	if err != nil {
		t.Fatalf("NewManager: %v", err)
	}

	// Store something before close.
	ctx := context.Background()
	if err := mgr.Store(ctx, &memory.Entry{Type: memory.MemoryLongTerm, Content: "test"}); err != nil {
		t.Fatalf("Store: %v", err)
	}

	if err := mgr.Close(); err != nil {
		t.Fatalf("Close: %v", err)
	}

	// After close, operations should be no-ops (enabled=false).
	if mgr.IsEnabled() {
		t.Error("expected IsEnabled() == false after Close()")
	}

	// Store after close should return nil error (no-op) per the implementation.
	if err := mgr.Store(ctx, &memory.Entry{Content: "after close"}); err != nil {
		t.Errorf("Store after close should be no-op: %v", err)
	}
}

func TestManager_EmptyQuery(t *testing.T) {
	mgr := newTestManager(t)
	ctx := context.Background()

	_ = mgr.Store(ctx, &memory.Entry{Type: memory.MemoryLongTerm, Content: "some data"})

	result, err := mgr.Query(ctx, "", 10, nil)
	if err != nil {
		t.Fatalf("Query with empty string: %v", err)
	}
	if result.Total != 0 {
		t.Errorf("expected 0 results for empty query, got %d", result.Total)
	}
}

// --- Additional coverage helpers ---

func TestManager_NewManager_NilConfig(t *testing.T) {
	tmp := t.TempDir()
	mgr, err := memory.NewManager(nil, tmp)
	if err != nil {
		t.Fatalf("NewManager with nil config: %v", err)
	}
	mgr.Close()
}

func TestManager_NewManager_InvalidWorkspace(t *testing.T) {
	// Use a path that cannot be created.
	_, err := memory.NewManager(nil, filepath.Join(string([]byte{0}), "invalid"))
	if err == nil {
		t.Fatal("expected error for invalid workspace path")
	}
}

func TestManager_GetNonExistent(t *testing.T) {
	mgr := newTestManager(t)
	ctx := context.Background()

	got, err := mgr.Get(ctx, "does-not-exist")
	if err != nil {
		t.Fatalf("Get non-existent: %v", err)
	}
	if got != nil {
		t.Error("expected nil for non-existent entry")
	}
}

func TestManager_Persistence(t *testing.T) {
	tmp := t.TempDir()

	// Create manager, store entry, close.
	mgr1, err := memory.NewManager(nil, tmp)
	if err != nil {
		t.Fatalf("NewManager: %v", err)
	}
	ctx := context.Background()
	entry := &memory.Entry{Type: memory.MemoryLongTerm, Content: "persistent data"}
	if err := mgr1.Store(ctx, entry); err != nil {
		t.Fatalf("Store: %v", err)
	}
	id := entry.ID
	mgr1.Close()

	// Create new manager pointing to the same directory.
	mgr2, err := memory.NewManager(nil, tmp)
	if err != nil {
		t.Fatalf("NewManager (second): %v", err)
	}
	defer mgr2.Close()

	got, err := mgr2.Get(ctx, id)
	if err != nil {
		t.Fatalf("Get after reload: %v", err)
	}
	if got == nil {
		t.Fatal("expected entry to survive persistence")
	}
	if got.Content != "persistent data" {
		t.Errorf("Content mismatch after reload: got %q", got.Content)
	}
}

func TestManager_ConcurrentAccess(t *testing.T) {
	mgr := newTestManager(t)
	ctx := context.Background()

	done := make(chan struct{})

	// Concurrent stores.
	for i := 0; i < 10; i++ {
		go func(n int) {
			defer func() { done <- struct{}{} }()
			_ = mgr.Store(ctx, &memory.Entry{
				Type:    memory.MemoryLongTerm,
				Content: "concurrent entry",
			})
		}(i)
	}
	for i := 0; i < 10; i++ {
		<-done
	}

	// Verify all entries are stored.
	result, err := mgr.List(ctx, nil, 0, 100)
	if err != nil {
		t.Fatalf("List: %v", err)
	}
	if result.Total != 10 {
		t.Errorf("expected 10 entries after concurrent stores, got %d", result.Total)
	}
}

func TestMemoryType_String(t *testing.T) {
	tests := []struct {
		mt   memory.MemoryType
		want string
	}{
		{memory.MemoryShortTerm, "short_term"},
		{memory.MemoryLongTerm, "long_term"},
		{memory.MemoryEpisodic, "episodic"},
		{memory.MemoryGraph, "graph"},
		{memory.MemoryDaily, "daily"},
		{memory.MemoryType(99), "unknown"},
	}
	for _, tt := range tests {
		if got := tt.mt.String(); got != tt.want {
			t.Errorf("MemoryType(%d).String() = %q, want %q", tt.mt, got, tt.want)
		}
	}
}

func TestParseMemoryType(t *testing.T) {
	tests := []struct {
		input string
		want  memory.MemoryType
	}{
		{"short_term", memory.MemoryShortTerm},
		{"long_term", memory.MemoryLongTerm},
		{"episodic", memory.MemoryEpisodic},
		{"graph", memory.MemoryGraph},
		{"daily", memory.MemoryDaily},
		{"unknown", memory.MemoryLongTerm}, // default
	}
	for _, tt := range tests {
		if got := memory.ParseMemoryType(tt.input); got != tt.want {
			t.Errorf("ParseMemoryType(%q) = %v, want %v", tt.input, got, tt.want)
		}
	}
}

func TestManager_QueryWithTags(t *testing.T) {
	mgr := newTestManager(t)
	ctx := context.Background()

	_ = mgr.Store(ctx, &memory.Entry{
		Type:    memory.MemoryLongTerm,
		Content: "some content",
		Tags:    []string{"golang", "testing"},
	})

	result, err := mgr.Query(ctx, "golang", 10, nil)
	if err != nil {
		t.Fatalf("Query: %v", err)
	}
	if result.Total != 1 {
		t.Errorf("expected 1 result matching tag 'golang', got %d", result.Total)
	}
}

func TestManager_QueryWithMetadata(t *testing.T) {
	mgr := newTestManager(t)
	ctx := context.Background()

	_ = mgr.Store(ctx, &memory.Entry{
		Type:    memory.MemoryEpisodic,
		Content: "discussed project architecture",
		Metadata: map[string]string{
			"project": "nemesisbot",
		},
	})

	result, err := mgr.Query(ctx, "nemesisbot", 10, nil)
	if err != nil {
		t.Fatalf("Query: %v", err)
	}
	if result.Total != 1 {
		t.Errorf("expected 1 result matching metadata 'nemesisbot', got %d", result.Total)
	}
}

func TestManager_StoreMultipleAndListSorted(t *testing.T) {
	mgr := newTestManager(t)
	ctx := context.Background()

	contents := []string{"alpha", "beta", "gamma"}
	for _, c := range contents {
		_ = mgr.Store(ctx, &memory.Entry{Type: memory.MemoryLongTerm, Content: c})
	}

	result, err := mgr.List(ctx, nil, 0, 10)
	if err != nil {
		t.Fatalf("List: %v", err)
	}
	if result.Total != 3 {
		t.Fatalf("expected 3 total, got %d", result.Total)
	}

	// Verify all entries are present.
	found := make(map[string]bool)
	for _, e := range result.Entries {
		found[e.Content] = true
	}
	for _, c := range contents {
		if !found[c] {
			t.Errorf("missing entry with content %q", c)
		}
	}
}

// Ensure the JSONL file is created on disk.
func TestManager_FileCreated(t *testing.T) {
	tmp := t.TempDir()
	mgr, err := memory.NewManager(nil, tmp)
	if err != nil {
		t.Fatalf("NewManager: %v", err)
	}

	ctx := context.Background()
	_ = mgr.Store(ctx, &memory.Entry{Type: memory.MemoryLongTerm, Content: "disk test"})
	mgr.Close()

	storeFile := filepath.Join(tmp, "memory", "store.jsonl")
	if _, err := os.Stat(storeFile); os.IsNotExist(err) {
		t.Errorf("expected store file at %s to exist", storeFile)
	}
}
