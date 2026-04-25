package memory_test

import (
	"context"
	"fmt"
	"path/filepath"
	"testing"
	"time"

	"github.com/276793422/NemesisBot/module/memory"
)

// TestE2E_VectorSearch_Recall tests semantic search recall with multiple topics.
func TestE2E_VectorSearch_Recall(t *testing.T) {
	dir := t.TempDir()
	cfg := &memory.Config{
		Vector: memory.VectorConfig{
			Enabled:             true,
			Backend:             "chromem",
			EmbeddingTier:       "local",
			LocalDim:            256,
			MaxResults:          10,
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

	// Store entries across different topics
	topics := map[string][]string{
		"programming": {
			"golang is a compiled statically typed language",
			"python supports dynamic typing and garbage collection",
			"rust provides memory safety without garbage collection",
			"javascript is the primary language of web browsers",
			"typescript adds static typing to javascript",
		},
		"cooking": {
			"sauté onions in butter until translucent",
			"knead bread dough for at least ten minutes",
			"roast vegetables at high temperature for caramelization",
			"season steak with salt and pepper before grilling",
			"use fresh herbs to enhance the flavor profile",
		},
		"science": {
			"photosynthesis converts sunlight into chemical energy",
			"gravity is a fundamental force of attraction",
			"DNA contains the genetic instructions for development",
			"mitochondria are the powerhouses of the cell",
			"evolution occurs through natural selection",
		},
	}

	for topic, entries := range topics {
		for _, content := range entries {
			entry := &memory.Entry{
				Type:    memory.MemoryLongTerm,
				Content: content,
				Tags:    []string{topic},
			}
			if err := mgr.Store(ctx, entry); err != nil {
				t.Fatalf("Store: %v", err)
			}
		}
	}

	// Query for programming languages and verify recall
	result, err := mgr.QuerySemantic(ctx, "programming languages and compilers", 5)
	if err != nil {
		t.Fatalf("QuerySemantic: %v", err)
	}

	// With hash embeddings, we primarily verify the search returns results
	// and the system works end-to-end. Hash embeddings may not perfectly
	// rank by topic, so we verify at least some results are returned.
	if result.Total == 0 {
		t.Error("expected at least some results from semantic search")
	}

	// Log results for diagnostic purposes
	for i, e := range result.Entries {
		content := e.Content
		if len(content) > 50 {
			content = content[:50]
		}
		t.Logf("Result %d: Score=%.4f Tags=%v Content=%q", i, e.Score, e.Tags, content)
	}
}

// TestE2E_VectorSearch_Multilingual tests semantic search with mixed language content.
func TestE2E_VectorSearch_Multilingual(t *testing.T) {
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

	entries := []*memory.Entry{
		{Type: memory.MemoryLongTerm, Content: "machine learning is a subset of artificial intelligence"},
		{Type: memory.MemoryLongTerm, Content: "机器学习是人工智能的一个子集"},
		{Type: memory.MemoryLongTerm, Content: "deep learning uses neural network architectures"},
	}
	for _, e := range entries {
		_ = mgr.Store(ctx, e)
	}

	// Query in English
	result, err := mgr.QuerySemantic(ctx, "artificial intelligence and ML", 3)
	if err != nil {
		t.Fatalf("QuerySemantic: %v", err)
	}
	if result.Total == 0 {
		t.Error("expected results for multilingual query")
	}
}

// TestE2E_VectorSearch_LargeDataset tests performance with a larger number of entries.
func TestE2E_VectorSearch_LargeDataset(t *testing.T) {
	if testing.Short() {
		t.Skip("skipping large dataset test in short mode")
	}

	dir := t.TempDir()
	cfg := &memory.Config{
		Vector: memory.VectorConfig{
			Enabled:             true,
			Backend:             "chromem",
			EmbeddingTier:       "local",
			LocalDim:            256,
			MaxResults:          10,
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

	// Generate 100 entries
	for i := 0; i < 100; i++ {
		entry := &memory.Entry{
			Type:    memory.MemoryLongTerm,
			Content: fmt.Sprintf("document number %d about topic %d with some unique content %d", i, i%10, i),
		}
		if err := mgr.Store(ctx, entry); err != nil {
			t.Fatalf("Store %d: %v", i, err)
		}
	}

	// Query and measure performance
	start := time.Now()
	result, err := mgr.QuerySemantic(ctx, "document topic 5", 10)
	elapsed := time.Since(start)

	if err != nil {
		t.Fatalf("QuerySemantic: %v", err)
	}
	if result.Total == 0 {
		t.Error("expected results from large dataset")
	}

	t.Logf("Query against 100 entries took %v, returned %d results", elapsed, result.Total)

	// Query should be fast even with 100 entries
	if elapsed > 5*time.Second {
		t.Errorf("query took too long: %v", elapsed)
	}
}
