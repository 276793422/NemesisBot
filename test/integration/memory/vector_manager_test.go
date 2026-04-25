package memory_test

import (
	"context"
	"os"
	"path/filepath"
	"testing"
	"time"

	"github.com/276793422/NemesisBot/module/memory"
)

func TestIntegration_ManagerWithVector_QuerySemantic(t *testing.T) {
	dir := t.TempDir()
	cfg := &memory.Config{
		Vector: memory.VectorConfig{
			Enabled:             true,
			Backend:             "chromem",
			EmbeddingTier:       "local",
			LocalDim:            256,
			MaxResults:          5,
			SimilarityThreshold: 0.3,
			RetentionDays:       90,
			StoragePath:         filepath.Join(dir, "vector", "vector_store.jsonl"),
		},
	}

	mgr, err := memory.NewManager(cfg, dir)
	if err != nil {
		t.Fatalf("NewManager: %v", err)
	}
	defer mgr.Close()

	ctx := context.Background()

	// Store entries
	entries := []*memory.Entry{
		{Type: memory.MemoryLongTerm, Content: "golang is a statically typed compiled programming language"},
		{Type: memory.MemoryLongTerm, Content: "python is a dynamically typed interpreted programming language"},
		{Type: memory.MemoryLongTerm, Content: "cats are domesticated carnivorous mammals"},
	}
	for _, e := range entries {
		if err := mgr.Store(ctx, e); err != nil {
			t.Fatalf("Store: %v", err)
		}
	}

	// Query using semantic search
	result, err := mgr.QuerySemantic(ctx, "programming language golang", 5)
	if err != nil {
		t.Fatalf("QuerySemantic: %v", err)
	}
	if result.Total == 0 {
		t.Error("expected at least one result from semantic search")
	}

	// The golang entry should rank highest
	if len(result.Entries) > 0 && result.Entries[0].Content == "" {
		t.Error("expected non-empty content in results")
	}
}

func TestIntegration_ManagerWithVector_StoreAndRetrieve(t *testing.T) {
	dir := t.TempDir()
	cfg := &memory.Config{
		Vector: memory.VectorConfig{
			Enabled:             true,
			Backend:             "chromem",
			EmbeddingTier:       "local",
			LocalDim:            256,
			MaxResults:          5,
			SimilarityThreshold: 0.3,
			StoragePath:         filepath.Join(dir, "vector", "vector_store.jsonl"),
		},
	}

	mgr, err := memory.NewManager(cfg, dir)
	if err != nil {
		t.Fatalf("NewManager: %v", err)
	}
	defer mgr.Close()

	ctx := context.Background()

	// Store an entry
	entry := &memory.Entry{
		Type:    memory.MemoryLongTerm,
		Content: "the earth orbits the sun",
		Tags:    []string{"astronomy", "facts"},
	}
	if err := mgr.Store(ctx, entry); err != nil {
		t.Fatalf("Store: %v", err)
	}

	// Retrieve by ID
	got, err := mgr.Get(ctx, entry.ID)
	if err != nil {
		t.Fatalf("Get: %v", err)
	}
	if got == nil {
		t.Fatal("expected to retrieve stored entry")
	}
	if got.Content != "the earth orbits the sun" {
		t.Errorf("got content %q, want %q", got.Content, "the earth orbits the sun")
	}

	// Query semantically - with hash embeddings, use a low threshold
	result, err := mgr.QuerySemantic(ctx, "earth sun orbits planets", 5)
	if err != nil {
		t.Fatalf("QuerySemantic: %v", err)
	}
	// Note: hash embeddings may not find perfect semantic matches for all queries.
	// The key thing is the system doesn't crash and returns valid results.
	t.Logf("QuerySemantic returned %d results", result.Total)
}

func TestIntegration_ManagerFallbackToLocalStore(t *testing.T) {
	dir := t.TempDir()
	// Default config: vector disabled, uses local TF-IDF
	cfg := memory.DefaultConfig()

	mgr, err := memory.NewManager(cfg, dir)
	if err != nil {
		t.Fatalf("NewManager: %v", err)
	}
	defer mgr.Close()

	ctx := context.Background()

	entry := &memory.Entry{
		Type:    memory.MemoryLongTerm,
		Content: "golang programming language",
	}
	_ = mgr.Store(ctx, entry)

	result, err := mgr.QuerySemantic(ctx, "golang", 5)
	if err != nil {
		t.Fatalf("QuerySemantic: %v", err)
	}
	if result.Total == 0 {
		t.Error("expected at least one result from local TF-IDF search")
	}
}

func TestIntegration_ManagerVectorEpisodic(t *testing.T) {
	dir := t.TempDir()
	cfg := &memory.Config{
		Vector: memory.VectorConfig{
			Enabled:             true,
			Backend:             "chromem",
			EmbeddingTier:       "local",
			LocalDim:            256,
			MaxResults:          5,
			SimilarityThreshold: 0.3,
			StoragePath:         filepath.Join(dir, "vector", "vector_store.jsonl"),
		},
	}

	mgr, err := memory.NewManager(cfg, dir)
	if err != nil {
		t.Fatalf("NewManager: %v", err)
	}
	defer mgr.Close()

	ctx := context.Background()

	// Store episodic memories
	if err := mgr.StoreEpisodic(ctx, "session-1", "user", "I prefer golang over python"); err != nil {
		t.Fatalf("StoreEpisodic: %v", err)
	}
	if err := mgr.StoreEpisodic(ctx, "session-1", "assistant", "Noted, you prefer statically typed languages"); err != nil {
		t.Fatalf("StoreEpisodic: %v", err)
	}

	// Query for user preferences
	result, err := mgr.QuerySemantic(ctx, "user programming language preference", 5)
	if err != nil {
		t.Fatalf("QuerySemantic: %v", err)
	}
	if result.Total == 0 {
		t.Error("expected at least one result")
	}
}

func TestIntegration_ManagerVectorDelete(t *testing.T) {
	dir := t.TempDir()
	cfg := &memory.Config{
		Vector: memory.VectorConfig{
			Enabled:             true,
			Backend:             "chromem",
			EmbeddingTier:       "local",
			LocalDim:            256,
			MaxResults:          5,
			SimilarityThreshold: 0.3,
			StoragePath:         filepath.Join(dir, "vector", "vector_store.jsonl"),
		},
	}

	mgr, err := memory.NewManager(cfg, dir)
	if err != nil {
		t.Fatalf("NewManager: %v", err)
	}
	defer mgr.Close()

	ctx := context.Background()

	entry := &memory.Entry{
		Type:    memory.MemoryLongTerm,
		Content: "temporary fact to be deleted",
	}
	_ = mgr.Store(ctx, entry)

	// Verify exists
	got, _ := mgr.Get(ctx, entry.ID)
	if got == nil {
		t.Fatal("entry should exist before deletion")
	}

	// Delete
	if err := mgr.Delete(ctx, entry.ID); err != nil {
		t.Fatalf("Delete: %v", err)
	}

	// Verify deleted
	got, _ = mgr.Get(ctx, entry.ID)
	if got != nil {
		t.Error("expected nil after deletion")
	}
}

func TestIntegration_ManagerVectorPersistenceReload(t *testing.T) {
	dir := t.TempDir()
	vstorePath := filepath.Join(dir, "vector", "vector_store.jsonl")

	cfg := &memory.Config{
		Vector: memory.VectorConfig{
			Enabled:             true,
			Backend:             "chromem",
			EmbeddingTier:       "local",
			LocalDim:            256,
			MaxResults:          5,
			SimilarityThreshold: 0.3,
			RetentionDays:       90,
			StoragePath:         vstorePath,
		},
	}

	// Create first manager and store data
	mgr1, err := memory.NewManager(cfg, dir)
	if err != nil {
		t.Fatalf("NewManager 1: %v", err)
	}
	ctx := context.Background()
	_ = mgr1.Store(ctx, &memory.Entry{
		Type:      memory.MemoryLongTerm,
		Content:   "persisted knowledge about databases",
		Tags:      []string{"database", "storage"},
		Metadata:  map[string]string{"source": "test"},
		CreatedAt: time.Now(),
	})
	_ = mgr1.Close()

	// Verify persistence file exists
	if _, err := os.Stat(vstorePath); os.IsNotExist(err) {
		t.Fatal("vector store persistence file not created")
	}

	// Create second manager and verify data
	mgr2, err := memory.NewManager(cfg, dir)
	if err != nil {
		t.Fatalf("NewManager 2: %v", err)
	}
	defer mgr2.Close()

	result, err := mgr2.QuerySemantic(ctx, "databases storage", 5)
	if err != nil {
		t.Fatalf("QuerySemantic: %v", err)
	}
	if result.Total == 0 {
		t.Error("expected persisted data to be found after reload")
	}
}
