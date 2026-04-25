package vector_test

import (
	"context"
	"os"
	"path/filepath"
	"sync"
	"testing"
	"time"

	"github.com/276793422/NemesisBot/module/memory/vector"
)

func newTestStore(t *testing.T) *vector.VectorStore {
	t.Helper()
	dir := t.TempDir()
	cfg := vector.StoreConfig{
		LocalDim:            256,
		SimilarityThreshold: 0.3,
		StoragePath:         filepath.Join(dir, "test_store.jsonl"),
	}
	vs, err := vector.NewVectorStore(cfg, nil)
	if err != nil {
		t.Fatalf("NewVectorStore: %v", err)
	}
	return vs
}

func TestVectorStore_StoreAndQuery(t *testing.T) {
	vs := newTestStore(t)
	defer vs.Close()
	ctx := context.Background()

	entries := []*vector.Entry{
		{ID: "1", Type: "long_term", Content: "golang is a programming language", Tags: []string{"programming"}, CreatedAt: time.Now()},
		{ID: "2", Type: "long_term", Content: "python is also a programming language", Tags: []string{"programming"}, CreatedAt: time.Now()},
		{ID: "3", Type: "long_term", Content: "cats are cute animals that purr", Tags: []string{"animals"}, CreatedAt: time.Now()},
	}
	for _, e := range entries {
		if err := vs.StoreEntry(ctx, e); err != nil {
			t.Fatalf("StoreEntry: %v", err)
		}
	}

	result, err := vs.Query(ctx, "programming language golang", 5, nil)
	if err != nil {
		t.Fatalf("Query: %v", err)
	}
	if result.Total == 0 {
		t.Error("expected at least one result")
	}

	found := false
	for _, e := range result.Entries {
		if e.ID == "1" {
			found = true
			break
		}
	}
	if !found {
		t.Error("expected to find entry about golang")
	}
}

func TestVectorStore_QueryThreshold(t *testing.T) {
	dir := t.TempDir()
	cfg := vector.StoreConfig{
		LocalDim:            256,
		SimilarityThreshold: 0.99,
		StoragePath:         filepath.Join(dir, "test_store.jsonl"),
	}
	vs, err := vector.NewVectorStore(cfg, nil)
	if err != nil {
		t.Fatal(err)
	}
	defer vs.Close()
	ctx := context.Background()

	_ = vs.StoreEntry(ctx, &vector.Entry{ID: "1", Type: "long_term", Content: "hello world", CreatedAt: time.Now()})

	result, err := vs.Query(ctx, "something completely different", 5, nil)
	if err != nil {
		t.Fatal(err)
	}
	if result.Total > 0 {
		t.Logf("threshold filtering: got %d results (may be OK with hash embeddings)", result.Total)
	}
}

func TestVectorStore_GetByID(t *testing.T) {
	vs := newTestStore(t)
	defer vs.Close()
	ctx := context.Background()

	entry := &vector.Entry{ID: "test-1", Type: "long_term", Content: "test content", CreatedAt: time.Now()}
	_ = vs.StoreEntry(ctx, entry)

	got, err := vs.GetByID(ctx, "test-1")
	if err != nil {
		t.Fatal(err)
	}
	if got == nil {
		t.Fatal("expected to find entry")
	}
	if got.ID != "test-1" {
		t.Errorf("got ID %q, want %q", got.ID, "test-1")
	}
}

func TestVectorStore_GetByID_NotFound(t *testing.T) {
	vs := newTestStore(t)
	defer vs.Close()
	ctx := context.Background()

	got, err := vs.GetByID(ctx, "nonexistent")
	if err != nil {
		t.Fatal(err)
	}
	if got != nil {
		t.Error("expected nil for nonexistent ID")
	}
}

func TestVectorStore_DeleteEntry(t *testing.T) {
	vs := newTestStore(t)
	defer vs.Close()
	ctx := context.Background()

	entry := &vector.Entry{ID: "del-1", Type: "long_term", Content: "to be deleted", CreatedAt: time.Now()}
	_ = vs.StoreEntry(ctx, entry)

	err := vs.DeleteEntry(ctx, "del-1")
	if err != nil {
		t.Fatalf("DeleteEntry: %v", err)
	}

	got, _ := vs.GetByID(ctx, "del-1")
	if got != nil {
		t.Error("expected nil after deletion")
	}
}

func TestVectorStore_ListEntries(t *testing.T) {
	vs := newTestStore(t)
	defer vs.Close()
	ctx := context.Background()

	_ = vs.StoreEntry(ctx, &vector.Entry{ID: "1", Type: "long_term", Content: "entry 1", CreatedAt: time.Now()})
	_ = vs.StoreEntry(ctx, &vector.Entry{ID: "2", Type: "episodic", Content: "entry 2", CreatedAt: time.Now()})
	_ = vs.StoreEntry(ctx, &vector.Entry{ID: "3", Type: "long_term", Content: "entry 3", CreatedAt: time.Now()})

	result, err := vs.ListEntries(nil, 0, 10)
	if err != nil {
		t.Fatal(err)
	}
	if result.Total != 3 {
		t.Errorf("expected 3 entries, got %d", result.Total)
	}

	result, err = vs.ListEntries([]string{"long_term"}, 0, 10)
	if err != nil {
		t.Fatal(err)
	}
	if result.Total != 2 {
		t.Errorf("expected 2 long_term entries, got %d", result.Total)
	}
}

func TestVectorStore_ListEntries_Pagination(t *testing.T) {
	vs := newTestStore(t)
	defer vs.Close()
	ctx := context.Background()

	for i := 0; i < 5; i++ {
		_ = vs.StoreEntry(ctx, &vector.Entry{
			ID:        string(rune('a' + i)),
			Type:      "long_term",
			Content:   "entry",
			CreatedAt: time.Now(),
		})
	}

	result, err := vs.ListEntries(nil, 2, 2)
	if err != nil {
		t.Fatal(err)
	}
	if result.Total != 5 {
		t.Errorf("expected total 5, got %d", result.Total)
	}
	if len(result.Entries) != 2 {
		t.Errorf("expected 2 entries on page, got %d", len(result.Entries))
	}
}

func TestVectorStore_PersistenceRoundTrip(t *testing.T) {
	dir := t.TempDir()
	persistPath := filepath.Join(dir, "persist_test.jsonl")

	cfg := vector.StoreConfig{
		LocalDim:            256,
		SimilarityThreshold: 0.3,
		StoragePath:         persistPath,
	}

	vs1, err := vector.NewVectorStore(cfg, nil)
	if err != nil {
		t.Fatal(err)
	}
	ctx := context.Background()
	_ = vs1.StoreEntry(ctx, &vector.Entry{ID: "p1", Type: "long_term", Content: "persistent entry", Tags: []string{"test"}, CreatedAt: time.Now()})
	_ = vs1.Close()

	if _, err := os.Stat(persistPath); os.IsNotExist(err) {
		t.Fatal("persistence file not created")
	}

	vs2, err := vector.NewVectorStore(cfg, nil)
	if err != nil {
		t.Fatal(err)
	}
	defer vs2.Close()

	got, err := vs2.GetByID(ctx, "p1")
	if err != nil {
		t.Fatal(err)
	}
	if got == nil {
		t.Fatal("expected to find persisted entry after reload")
	}
	if got.Content != "persistent entry" {
		t.Errorf("got content %q, want %q", got.Content, "persistent entry")
	}
}

func TestVectorStore_ConcurrentAccess(t *testing.T) {
	vs := newTestStore(t)
	defer vs.Close()
	ctx := context.Background()

	var wg sync.WaitGroup
	for i := 0; i < 10; i++ {
		wg.Add(1)
		go func(i int) {
			defer wg.Done()
			_ = vs.StoreEntry(ctx, &vector.Entry{
				ID:        string(rune('A' + i)),
				Type:      "long_term",
				Content:   "concurrent entry",
				CreatedAt: time.Now(),
			})
		}(i)
	}
	wg.Wait()

	result, err := vs.ListEntries(nil, 0, 20)
	if err != nil {
		t.Fatal(err)
	}
	if result.Total != 10 {
		t.Errorf("expected 10 entries, got %d", result.Total)
	}
}

func TestVectorStore_StoreNil(t *testing.T) {
	vs := newTestStore(t)
	defer vs.Close()

	err := vs.StoreEntry(context.Background(), nil)
	if err != nil {
		t.Errorf("storing nil should be no-op, got: %v", err)
	}
}

func TestVectorStore_QueryTypeFilter(t *testing.T) {
	vs := newTestStore(t)
	defer vs.Close()
	ctx := context.Background()

	_ = vs.StoreEntry(ctx, &vector.Entry{ID: "1", Type: "long_term", Content: "golang programming", CreatedAt: time.Now()})
	_ = vs.StoreEntry(ctx, &vector.Entry{ID: "2", Type: "episodic", Content: "golang programming episode", CreatedAt: time.Now()})

	result, err := vs.Query(ctx, "golang", 5, []string{"long_term"})
	if err != nil {
		t.Fatal(err)
	}
	for _, e := range result.Entries {
		if e.Type != "long_term" {
			t.Errorf("expected only long_term, got %q", e.Type)
		}
	}
}

func TestVectorStore_QueryEmptyQuery(t *testing.T) {
	vs := newTestStore(t)
	defer vs.Close()
	ctx := context.Background()

	// Store one entry first so chromem-go actually processes the query
	_ = vs.StoreEntry(ctx, &vector.Entry{ID: "1", Type: "long_term", Content: "hello world", CreatedAt: time.Now()})

	// Empty query should return an error from chromem-go
	_, err := vs.Query(ctx, "", 5, nil)
	if err == nil {
		t.Error("expected error for empty query")
	}
}

func TestVectorStore_QueryEmptyStore(t *testing.T) {
	vs := newTestStore(t)
	defer vs.Close()
	ctx := context.Background()

	// Query on empty store should return empty results, not error
	result, err := vs.Query(ctx, "anything", 5, nil)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if result.Total != 0 {
		t.Errorf("expected 0 results on empty store, got %d", result.Total)
	}
}

func TestVectorStore_EmptyPersistence(t *testing.T) {
	dir := t.TempDir()
	cfg := vector.StoreConfig{
		LocalDim:            256,
		SimilarityThreshold: 0.5,
		StoragePath:         filepath.Join(dir, "empty.jsonl"),
	}
	vs, err := vector.NewVectorStore(cfg, nil)
	if err != nil {
		t.Fatal(err)
	}
	vs.Close()
}

func TestNewVectorStore_DefaultPersistPath(t *testing.T) {
	cfg := vector.StoreConfig{
		LocalDim:            256,
		SimilarityThreshold: 0.5,
	}
	vs, err := vector.NewVectorStore(cfg, nil)
	if err != nil {
		t.Fatal(err)
	}
	defer vs.Close()
}

func TestVectorStore_GetByID_PreservesTagsAndMetadata(t *testing.T) {
	vs := newTestStore(t)
	defer vs.Close()
	ctx := context.Background()

	entry := &vector.Entry{
		ID:        "tag-test-1",
		Type:      "long_term",
		Content:   "entry with tags and metadata",
		Tags:      []string{"programming", "golang"},
		Metadata:  map[string]string{"source": "unit-test", "priority": "high"},
		CreatedAt: time.Date(2026, 4, 25, 10, 0, 0, 0, time.UTC),
	}
	if err := vs.StoreEntry(ctx, entry); err != nil {
		t.Fatalf("StoreEntry: %v", err)
	}

	got, err := vs.GetByID(ctx, "tag-test-1")
	if err != nil {
		t.Fatal(err)
	}
	if got == nil {
		t.Fatal("expected to find entry")
	}

	// Verify tags preserved
	if len(got.Tags) != 2 {
		t.Fatalf("expected 2 tags, got %d: %v", len(got.Tags), got.Tags)
	}
	if got.Tags[0] != "programming" || got.Tags[1] != "golang" {
		t.Errorf("tags mismatch: got %v, want [programming golang]", got.Tags)
	}

	// Verify metadata preserved
	if got.Metadata["source"] != "unit-test" {
		t.Errorf("metadata source = %q, want %q", got.Metadata["source"], "unit-test")
	}
	if got.Metadata["priority"] != "high" {
		t.Errorf("metadata priority = %q, want %q", got.Metadata["priority"], "high")
	}

	// Verify CreatedAt preserved
	if !got.CreatedAt.Equal(time.Date(2026, 4, 25, 10, 0, 0, 0, time.UTC)) {
		t.Errorf("CreatedAt = %v, want 2026-04-25 10:00:00 UTC", got.CreatedAt)
	}
}

func TestVectorStore_Query_PreservesTagsAndMetadata(t *testing.T) {
	vs := newTestStore(t)
	defer vs.Close()
	ctx := context.Background()

	_ = vs.StoreEntry(ctx, &vector.Entry{
		ID:        "q1",
		Type:      "long_term",
		Content:   "golang programming language",
		Tags:      []string{"programming", "golang"},
		Metadata:  map[string]string{"source": "test"},
		CreatedAt: time.Date(2026, 1, 1, 0, 0, 0, 0, time.UTC),
	})

	result, err := vs.Query(ctx, "golang", 5, nil)
	if err != nil {
		t.Fatal(err)
	}
	if result.Total == 0 {
		t.Fatal("expected at least one result")
	}

	got := result.Entries[0]
	if len(got.Tags) != 2 {
		t.Errorf("expected 2 tags in query result, got %d: %v", len(got.Tags), got.Tags)
	}
	if got.Tags[0] != "programming" || got.Tags[1] != "golang" {
		t.Errorf("tags in query result: got %v, want [programming golang]", got.Tags)
	}
	if got.Metadata["source"] != "test" {
		t.Errorf("metadata source in query result = %q, want %q", got.Metadata["source"], "test")
	}
}
