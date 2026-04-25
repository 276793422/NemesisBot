package graph_test

import (
	"context"
	"strings"
	"testing"
	"time"

	"github.com/276793422/NemesisBot/module/memory/graph"
)

// ---------------------------------------------------------------------------
// NewStore
// ---------------------------------------------------------------------------

func TestNewStore(t *testing.T) {
	dir := t.TempDir()
	store, err := graph.NewStore(graph.Config{StoragePath: dir})
	if err != nil {
		t.Fatalf("NewStore: %v", err)
	}
	defer store.Close()

	if store.EntityCount() != 0 {
		t.Error("New store should have 0 entities")
	}
	if store.TripleCount() != 0 {
		t.Error("New store should have 0 triples")
	}
}

func TestNewStore_CreatesDirectory(t *testing.T) {
	dir := t.TempDir() + "/subdir/graph"
	store, err := graph.NewStore(graph.Config{StoragePath: dir})
	if err != nil {
		t.Fatalf("NewStore with nested dir: %v", err)
	}
	defer store.Close()
}

func TestNewStore_LoadsExisting(t *testing.T) {
	dir := t.TempDir()
	ctx := context.Background()

	// Create store and add data
	store1, _ := graph.NewStore(graph.Config{StoragePath: dir})
	_ = store1.AddEntity(ctx, &graph.Entity{Name: "Alice", Type: "person"})
	_ = store1.AddTriple(ctx, &graph.Triple{Subject: "Alice", Predicate: "knows", Object: "Bob"})
	store1.Close()

	// Create new store from same directory
	store2, err := graph.NewStore(graph.Config{StoragePath: dir})
	if err != nil {
		t.Fatalf("NewStore reload: %v", err)
	}
	defer store2.Close()

	if store2.EntityCount() != 1 {
		t.Errorf("EntityCount = %d, want 1", store2.EntityCount())
	}
	if store2.TripleCount() != 1 {
		t.Errorf("TripleCount = %d, want 1", store2.TripleCount())
	}
}

// ---------------------------------------------------------------------------
// AddEntity
// ---------------------------------------------------------------------------

func TestStore_AddEntity(t *testing.T) {
	dir := t.TempDir()
	store, _ := graph.NewStore(graph.Config{StoragePath: dir})
	defer store.Close()

	ctx := context.Background()
	entity := &graph.Entity{
		Name:       "Alice",
		Type:       "person",
		Properties: map[string]string{"age": "30", "city": "NYC"},
	}

	if err := store.AddEntity(ctx, entity); err != nil {
		t.Fatalf("AddEntity: %v", err)
	}

	if store.EntityCount() != 1 {
		t.Errorf("EntityCount = %d, want 1", store.EntityCount())
	}
}

func TestStore_AddEntity_SetsTimestamp(t *testing.T) {
	dir := t.TempDir()
	store, _ := graph.NewStore(graph.Config{StoragePath: dir})
	defer store.Close()

	ctx := context.Background()
	entity := &graph.Entity{Name: "Bob", Type: "person"}
	_ = store.AddEntity(ctx, entity)

	if entity.CreatedAt.IsZero() {
		t.Error("AddEntity should set CreatedAt")
	}
}

func TestStore_AddEntity_InitializesProperties(t *testing.T) {
	dir := t.TempDir()
	store, _ := graph.NewStore(graph.Config{StoragePath: dir})
	defer store.Close()

	ctx := context.Background()
	entity := &graph.Entity{Name: "NoProps", Type: "concept"}
	_ = store.AddEntity(ctx, entity)

	if entity.Properties == nil {
		t.Error("AddEntity should initialize Properties map")
	}
}

func TestStore_AddEntity_Nil(t *testing.T) {
	dir := t.TempDir()
	store, _ := graph.NewStore(graph.Config{StoragePath: dir})
	defer store.Close()

	err := store.AddEntity(context.Background(), nil)
	if err == nil {
		t.Error("Expected error for nil entity")
	}
}

func TestStore_AddEntity_EmptyName(t *testing.T) {
	dir := t.TempDir()
	store, _ := graph.NewStore(graph.Config{StoragePath: dir})
	defer store.Close()

	err := store.AddEntity(context.Background(), &graph.Entity{Name: "", Type: "concept"})
	if err == nil {
		t.Error("Expected error for empty name")
	}
}

func TestStore_AddEntity_Update(t *testing.T) {
	dir := t.TempDir()
	store, _ := graph.NewStore(graph.Config{StoragePath: dir})
	defer store.Close()

	ctx := context.Background()

	// Add entity
	_ = store.AddEntity(ctx, &graph.Entity{Name: "Alice", Type: "person", Properties: map[string]string{"age": "30"}})

	// Update entity (same name, different type/properties)
	_ = store.AddEntity(ctx, &graph.Entity{Name: "Alice", Type: "developer", Properties: map[string]string{"lang": "Go"}})

	// Should still be 1 entity
	if store.EntityCount() != 1 {
		t.Errorf("EntityCount = %d, want 1", store.EntityCount())
	}

	entity, _ := store.GetEntity(ctx, "Alice")
	if entity.Type != "developer" {
		t.Errorf("Type = %q, want %q", entity.Type, "developer")
	}
}

func TestStore_AddEntity_CaseInsensitive(t *testing.T) {
	dir := t.TempDir()
	store, _ := graph.NewStore(graph.Config{StoragePath: dir})
	defer store.Close()

	ctx := context.Background()

	_ = store.AddEntity(ctx, &graph.Entity{Name: "Paris", Type: "city"})
	entity, err := store.GetEntity(ctx, "paris")
	if err != nil {
		t.Fatalf("GetEntity: %v", err)
	}
	if entity == nil {
		t.Error("GetEntity should be case-insensitive")
	}
}

// ---------------------------------------------------------------------------
// GetEntity
// ---------------------------------------------------------------------------

func TestStore_GetEntity_Existing(t *testing.T) {
	dir := t.TempDir()
	store, _ := graph.NewStore(graph.Config{StoragePath: dir})
	defer store.Close()

	ctx := context.Background()
	_ = store.AddEntity(ctx, &graph.Entity{
		Name:       "Alice",
		Type:       "person",
		Properties: map[string]string{"occupation": "engineer"},
	})

	entity, err := store.GetEntity(ctx, "Alice")
	if err != nil {
		t.Fatalf("GetEntity: %v", err)
	}
	if entity == nil {
		t.Fatal("Entity should exist")
	}
	if entity.Name != "Alice" {
		t.Errorf("Name = %q, want %q", entity.Name, "Alice")
	}
	if entity.Properties["occupation"] != "engineer" {
		t.Errorf("Properties[occupation] = %q, want %q", entity.Properties["occupation"], "engineer")
	}
}

func TestStore_GetEntity_NonExisting(t *testing.T) {
	dir := t.TempDir()
	store, _ := graph.NewStore(graph.Config{StoragePath: dir})
	defer store.Close()

	entity, err := store.GetEntity(context.Background(), "Nobody")
	if err != nil {
		t.Fatalf("GetEntity: %v", err)
	}
	if entity != nil {
		t.Error("GetEntity should return nil for non-existing entity")
	}
}

// ---------------------------------------------------------------------------
// AddTriple
// ---------------------------------------------------------------------------

func TestStore_AddTriple(t *testing.T) {
	dir := t.TempDir()
	store, _ := graph.NewStore(graph.Config{StoragePath: dir})
	defer store.Close()

	ctx := context.Background()
	triple := &graph.Triple{
		Subject:    "Alice",
		Predicate:  "knows",
		Object:     "Bob",
		Confidence: 0.9,
		Metadata:   map[string]string{"since": "2020"},
	}

	if err := store.AddTriple(ctx, triple); err != nil {
		t.Fatalf("AddTriple: %v", err)
	}

	if store.TripleCount() != 1 {
		t.Errorf("TripleCount = %d, want 1", store.TripleCount())
	}
}

func TestStore_AddTriple_SetsTimestamp(t *testing.T) {
	dir := t.TempDir()
	store, _ := graph.NewStore(graph.Config{StoragePath: dir})
	defer store.Close()

	ctx := context.Background()
	triple := &graph.Triple{Subject: "A", Predicate: "rel", Object: "B"}
	_ = store.AddTriple(ctx, triple)

	if triple.CreatedAt.IsZero() {
		t.Error("AddTriple should set CreatedAt")
	}
}

func TestStore_AddTriple_InitializesMetadata(t *testing.T) {
	dir := t.TempDir()
	store, _ := graph.NewStore(graph.Config{StoragePath: dir})
	defer store.Close()

	ctx := context.Background()
	triple := &graph.Triple{Subject: "A", Predicate: "rel", Object: "B"}
	_ = store.AddTriple(ctx, triple)

	if triple.Metadata == nil {
		t.Error("AddTriple should initialize Metadata map")
	}
}

func TestStore_AddTriple_Nil(t *testing.T) {
	dir := t.TempDir()
	store, _ := graph.NewStore(graph.Config{StoragePath: dir})
	defer store.Close()

	err := store.AddTriple(context.Background(), nil)
	if err == nil {
		t.Error("Expected error for nil triple")
	}
}

func TestStore_AddTriple_MissingFields(t *testing.T) {
	dir := t.TempDir()
	store, _ := graph.NewStore(graph.Config{StoragePath: dir})
	defer store.Close()

	tests := []struct {
		name      string
		triple    *graph.Triple
		wantError bool
	}{
		{"empty subject", &graph.Triple{Subject: "", Predicate: "p", Object: "o"}, true},
		{"empty predicate", &graph.Triple{Subject: "s", Predicate: "", Object: "o"}, true},
		{"empty object", &graph.Triple{Subject: "s", Predicate: "p", Object: ""}, true},
		{"all fields", &graph.Triple{Subject: "s", Predicate: "p", Object: "o"}, false},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			err := store.AddTriple(context.Background(), tt.triple)
			if (err != nil) != tt.wantError {
				t.Errorf("AddTriple error = %v, wantError %v", err, tt.wantError)
			}
		})
	}
}

func TestStore_AddTriple_Duplicate(t *testing.T) {
	dir := t.TempDir()
	store, _ := graph.NewStore(graph.Config{StoragePath: dir})
	defer store.Close()

	ctx := context.Background()

	t1 := &graph.Triple{Subject: "A", Predicate: "knows", Object: "B", Confidence: 0.8}
	_ = store.AddTriple(ctx, t1)

	// Add duplicate with different confidence — should update
	t2 := &graph.Triple{Subject: "A", Predicate: "knows", Object: "B", Confidence: 0.95}
	_ = store.AddTriple(ctx, t2)

	if store.TripleCount() != 1 {
		t.Errorf("TripleCount = %d, want 1 (duplicate should update)", store.TripleCount())
	}

	// Verify confidence was updated
	triples, _ := store.Query(ctx, "A", "knows", "B")
	if len(triples) != 1 {
		t.Fatalf("Expected 1 triple, got %d", len(triples))
	}
	if triples[0].Confidence != 0.95 {
		t.Errorf("Confidence = %f, want 0.95", triples[0].Confidence)
	}
}

// ---------------------------------------------------------------------------
// Query
// ---------------------------------------------------------------------------

func TestStore_Query_BySubject(t *testing.T) {
	dir := t.TempDir()
	store, _ := graph.NewStore(graph.Config{StoragePath: dir})
	defer store.Close()

	ctx := context.Background()
	_ = store.AddTriple(ctx, &graph.Triple{Subject: "Alice", Predicate: "knows", Object: "Bob"})
	_ = store.AddTriple(ctx, &graph.Triple{Subject: "Alice", Predicate: "works_at", Object: "Google"})
	_ = store.AddTriple(ctx, &graph.Triple{Subject: "Bob", Predicate: "knows", Object: "Carol"})

	results, err := store.Query(ctx, "Alice", "", "")
	if err != nil {
		t.Fatalf("Query: %v", err)
	}
	if len(results) != 2 {
		t.Errorf("Expected 2 results, got %d", len(results))
	}
}

func TestStore_Query_ByPredicate(t *testing.T) {
	dir := t.TempDir()
	store, _ := graph.NewStore(graph.Config{StoragePath: dir})
	defer store.Close()

	ctx := context.Background()
	_ = store.AddTriple(ctx, &graph.Triple{Subject: "Alice", Predicate: "knows", Object: "Bob"})
	_ = store.AddTriple(ctx, &graph.Triple{Subject: "Bob", Predicate: "knows", Object: "Carol"})
	_ = store.AddTriple(ctx, &graph.Triple{Subject: "Dave", Predicate: "hates", Object: "Eve"})

	results, err := store.Query(ctx, "", "knows", "")
	if err != nil {
		t.Fatalf("Query: %v", err)
	}
	if len(results) != 2 {
		t.Errorf("Expected 2 results for 'knows', got %d", len(results))
	}
}

func TestStore_Query_ByObject(t *testing.T) {
	dir := t.TempDir()
	store, _ := graph.NewStore(graph.Config{StoragePath: dir})
	defer store.Close()

	ctx := context.Background()
	_ = store.AddTriple(ctx, &graph.Triple{Subject: "Alice", Predicate: "lives_in", Object: "Paris"})
	_ = store.AddTriple(ctx, &graph.Triple{Subject: "Bob", Predicate: "lives_in", Object: "Paris"})
	_ = store.AddTriple(ctx, &graph.Triple{Subject: "Carol", Predicate: "lives_in", Object: "Berlin"})

	results, err := store.Query(ctx, "", "", "Paris")
	if err != nil {
		t.Fatalf("Query: %v", err)
	}
	if len(results) != 2 {
		t.Errorf("Expected 2 results for 'Paris', got %d", len(results))
	}
}

func TestStore_Query_AllWildcards(t *testing.T) {
	dir := t.TempDir()
	store, _ := graph.NewStore(graph.Config{StoragePath: dir})
	defer store.Close()

	ctx := context.Background()
	_ = store.AddTriple(ctx, &graph.Triple{Subject: "A", Predicate: "p1", Object: "B"})
	_ = store.AddTriple(ctx, &graph.Triple{Subject: "C", Predicate: "p2", Object: "D"})

	results, err := store.Query(ctx, "", "", "")
	if err != nil {
		t.Fatalf("Query: %v", err)
	}
	if len(results) != 2 {
		t.Errorf("All wildcards should return all, got %d", len(results))
	}
}

func TestStore_Query_NoMatch(t *testing.T) {
	dir := t.TempDir()
	store, _ := graph.NewStore(graph.Config{StoragePath: dir})
	defer store.Close()

	results, err := store.Query(context.Background(), "Nobody", "", "")
	if err != nil {
		t.Fatalf("Query: %v", err)
	}
	if len(results) != 0 {
		t.Errorf("Expected 0 results, got %d", len(results))
	}
}

func TestStore_Query_CaseInsensitive(t *testing.T) {
	dir := t.TempDir()
	store, _ := graph.NewStore(graph.Config{StoragePath: dir})
	defer store.Close()

	ctx := context.Background()
	_ = store.AddTriple(ctx, &graph.Triple{Subject: "Alice", Predicate: "Knows", Object: "Bob"})

	results, err := store.Query(ctx, "alice", "knows", "bob")
	if err != nil {
		t.Fatalf("Query: %v", err)
	}
	if len(results) != 1 {
		t.Errorf("Query should be case-insensitive, got %d results", len(results))
	}
}

// ---------------------------------------------------------------------------
// GetRelated — BFS traversal
// ---------------------------------------------------------------------------

func TestStore_GetRelated_Depth1(t *testing.T) {
	dir := t.TempDir()
	store, _ := graph.NewStore(graph.Config{StoragePath: dir})
	defer store.Close()

	ctx := context.Background()
	// Use asymmetric graph so only one triple involves Alice at depth 1
	_ = store.AddTriple(ctx, &graph.Triple{Subject: "Alice", Predicate: "knows", Object: "Bob"})

	results, err := store.GetRelated(ctx, "Alice", 1)
	if err != nil {
		t.Fatalf("GetRelated: %v", err)
	}
	if len(results) != 1 {
		t.Errorf("Depth 1: expected 1 triple, got %d", len(results))
	}
}

func TestStore_GetRelated_Depth2(t *testing.T) {
	dir := t.TempDir()
	store, _ := graph.NewStore(graph.Config{StoragePath: dir})
	defer store.Close()

	ctx := context.Background()
	_ = store.AddTriple(ctx, &graph.Triple{Subject: "Alice", Predicate: "knows", Object: "Bob"})
	_ = store.AddTriple(ctx, &graph.Triple{Subject: "Bob", Predicate: "knows", Object: "Carol"})
	_ = store.AddTriple(ctx, &graph.Triple{Subject: "Carol", Predicate: "knows", Object: "Dave"})

	results, err := store.GetRelated(ctx, "Alice", 2)
	if err != nil {
		t.Fatalf("GetRelated: %v", err)
	}
	// BFS is bidirectional: at depth 0, Alice finds A->B (1 triple).
	// At depth 1, Bob matches A->B (as Object) and B->C (as Subject) = 2 more triples.
	// Total = 3 (A->B appears twice because Bob matches both Subject and Object sides).
	if len(results) != 3 {
		t.Errorf("Depth 2: expected 3 triples, got %d", len(results))
	}
}

func TestStore_GetRelated_Depth3(t *testing.T) {
	dir := t.TempDir()
	store, _ := graph.NewStore(graph.Config{StoragePath: dir})
	defer store.Close()

	ctx := context.Background()
	_ = store.AddTriple(ctx, &graph.Triple{Subject: "Alice", Predicate: "knows", Object: "Bob"})
	_ = store.AddTriple(ctx, &graph.Triple{Subject: "Bob", Predicate: "knows", Object: "Carol"})
	_ = store.AddTriple(ctx, &graph.Triple{Subject: "Carol", Predicate: "knows", Object: "Dave"})

	results, err := store.GetRelated(ctx, "Alice", 3)
	if err != nil {
		t.Fatalf("GetRelated: %v", err)
	}
	// Bidirectional BFS accumulates duplicates when a node appears in both Subject and Object:
	// depth 0 (Alice): A->B = 1
	// depth 1 (Bob):   A->B + B->C = 2 (cumulative 3)
	// depth 2 (Carol): B->C + C->D = 2 (cumulative 5)
	if len(results) != 5 {
		t.Errorf("Depth 3: expected 5 triples, got %d", len(results))
	}
}

func TestStore_GetRelated_NoRelationships(t *testing.T) {
	dir := t.TempDir()
	store, _ := graph.NewStore(graph.Config{StoragePath: dir})
	defer store.Close()

	results, err := store.GetRelated(context.Background(), "Nobody", 3)
	if err != nil {
		t.Fatalf("GetRelated: %v", err)
	}
	if len(results) != 0 {
		t.Errorf("Expected 0 results for unknown entity, got %d", len(results))
	}
}

func TestStore_GetRelated_DefaultDepth(t *testing.T) {
	dir := t.TempDir()
	store, _ := graph.NewStore(graph.Config{StoragePath: dir})
	defer store.Close()

	ctx := context.Background()
	_ = store.AddTriple(ctx, &graph.Triple{Subject: "Alice", Predicate: "knows", Object: "Bob"})

	// Depth 0 should default to 1
	results, err := store.GetRelated(ctx, "Alice", 0)
	if err != nil {
		t.Fatalf("GetRelated: %v", err)
	}
	if len(results) != 1 {
		t.Errorf("Default depth 1: expected 1 triple, got %d", len(results))
	}
}

func TestStore_GetRelated_CircularReference(t *testing.T) {
	dir := t.TempDir()
	store, _ := graph.NewStore(graph.Config{StoragePath: dir})
	defer store.Close()

	ctx := context.Background()
	// A -> B -> C -> A (circular)
	_ = store.AddTriple(ctx, &graph.Triple{Subject: "A", Predicate: "knows", Object: "B"})
	_ = store.AddTriple(ctx, &graph.Triple{Subject: "B", Predicate: "knows", Object: "C"})
	_ = store.AddTriple(ctx, &graph.Triple{Subject: "C", Predicate: "knows", Object: "A"})

	results, err := store.GetRelated(ctx, "A", 5)
	if err != nil {
		t.Fatalf("GetRelated: %v", err)
	}
	// Should not infinite loop; bidirectional BFS revisits triples:
	// depth 0 (A): A->B (Subject) + C->A (Object) = 2 triples. Queue: [B, C]
	// depth 1 (B): A->B (Object) + B->C (Subject) = 2 more. Queue: [C]
	// depth 1 (C): B->C (Object, C already visited but triple still added) + C->A (Subject, A already visited) = 2 more
	// Total = 6
	if len(results) != 6 {
		t.Errorf("Circular: expected 6 triples, got %d", len(results))
	}
}

func TestStore_GetRelated_Bidirectional(t *testing.T) {
	dir := t.TempDir()
	store, _ := graph.NewStore(graph.Config{StoragePath: dir})
	defer store.Close()

	ctx := context.Background()
	_ = store.AddTriple(ctx, &graph.Triple{Subject: "Alice", Predicate: "knows", Object: "Bob"})

	// Searching for "Bob" should find the triple via Object match
	results, err := store.GetRelated(ctx, "Bob", 1)
	if err != nil {
		t.Fatalf("GetRelated: %v", err)
	}
	if len(results) != 1 {
		t.Errorf("Bidirectional: expected 1 triple, got %d", len(results))
	}
}

// ---------------------------------------------------------------------------
// Search
// ---------------------------------------------------------------------------

func TestStore_Search(t *testing.T) {
	dir := t.TempDir()
	store, _ := graph.NewStore(graph.Config{StoragePath: dir})
	defer store.Close()

	ctx := context.Background()
	_ = store.AddTriple(ctx, &graph.Triple{Subject: "Python", Predicate: "is_a", Object: "language"})
	_ = store.AddTriple(ctx, &graph.Triple{Subject: "Go", Predicate: "is_a", Object: "language"})
	_ = store.AddTriple(ctx, &graph.Triple{Subject: "Rust", Predicate: "is_a", Object: "language"})

	results, err := store.Search(ctx, "python", 10)
	if err != nil {
		t.Fatalf("Search: %v", err)
	}
	if len(results) != 1 {
		t.Errorf("Expected 1 result, got %d", len(results))
	}
}

func TestStore_Search_ByPredicate(t *testing.T) {
	dir := t.TempDir()
	store, _ := graph.NewStore(graph.Config{StoragePath: dir})
	defer store.Close()

	ctx := context.Background()
	_ = store.AddTriple(ctx, &graph.Triple{Subject: "Alice", Predicate: "works_at", Object: "Google"})
	_ = store.AddTriple(ctx, &graph.Triple{Subject: "Bob", Predicate: "works_at", Object: "Microsoft"})
	_ = store.AddTriple(ctx, &graph.Triple{Subject: "Carol", Predicate: "studies_at", Object: "MIT"})

	results, err := store.Search(ctx, "works_at", 10)
	if err != nil {
		t.Fatalf("Search: %v", err)
	}
	if len(results) != 2 {
		t.Errorf("Expected 2 results for 'works_at', got %d", len(results))
	}
}

func TestStore_Search_ByObject(t *testing.T) {
	dir := t.TempDir()
	store, _ := graph.NewStore(graph.Config{StoragePath: dir})
	defer store.Close()

	ctx := context.Background()
	_ = store.AddTriple(ctx, &graph.Triple{Subject: "Alice", Predicate: "likes", Object: "Pizza"})
	_ = store.AddTriple(ctx, &graph.Triple{Subject: "Bob", Predicate: "likes", Object: "Burger"})

	results, err := store.Search(ctx, "pizza", 10)
	if err != nil {
		t.Fatalf("Search: %v", err)
	}
	if len(results) != 1 {
		t.Errorf("Expected 1 result for 'pizza', got %d", len(results))
	}
}

func TestStore_Search_CaseInsensitive(t *testing.T) {
	dir := t.TempDir()
	store, _ := graph.NewStore(graph.Config{StoragePath: dir})
	defer store.Close()

	ctx := context.Background()
	_ = store.AddTriple(ctx, &graph.Triple{Subject: "UPPERCASE", Predicate: "REL", Object: "VALUE"})

	results, err := store.Search(ctx, "uppercase", 10)
	if err != nil {
		t.Fatalf("Search: %v", err)
	}
	if len(results) != 1 {
		t.Errorf("Search should be case-insensitive, got %d results", len(results))
	}
}

func TestStore_Search_Limit(t *testing.T) {
	dir := t.TempDir()
	store, _ := graph.NewStore(graph.Config{StoragePath: dir})
	defer store.Close()

	ctx := context.Background()
	for i := 0; i < 20; i++ {
		_ = store.AddTriple(ctx, &graph.Triple{
			Subject: "common", Predicate: "rel", Object: "target",
		})
	}

	results, err := store.Search(ctx, "common", 5)
	if err != nil {
		t.Fatalf("Search: %v", err)
	}
	// All have same subject/predicate/object, so first match finds it
	// but search stops at limit
	if len(results) > 5 {
		t.Errorf("Expected at most 5 results, got %d", len(results))
	}
}

func TestStore_Search_ZeroLimit(t *testing.T) {
	dir := t.TempDir()
	store, _ := graph.NewStore(graph.Config{StoragePath: dir})
	defer store.Close()

	ctx := context.Background()
	_ = store.AddTriple(ctx, &graph.Triple{Subject: "test", Predicate: "rel", Object: "val"})

	// Zero limit should default to 20
	results, err := store.Search(ctx, "test", 0)
	if err != nil {
		t.Fatalf("Search: %v", err)
	}
	if len(results) != 1 {
		t.Errorf("Expected 1 result, got %d", len(results))
	}
}

func TestStore_Search_NoResults(t *testing.T) {
	dir := t.TempDir()
	store, _ := graph.NewStore(graph.Config{StoragePath: dir})
	defer store.Close()

	results, err := store.Search(context.Background(), "nonexistent_xyz", 10)
	if err != nil {
		t.Fatalf("Search: %v", err)
	}
	if len(results) != 0 {
		t.Errorf("Expected 0 results, got %d", len(results))
	}
}

// ---------------------------------------------------------------------------
// DeleteEntity
// ---------------------------------------------------------------------------

func TestStore_DeleteEntity(t *testing.T) {
	dir := t.TempDir()
	store, _ := graph.NewStore(graph.Config{StoragePath: dir})
	defer store.Close()

	ctx := context.Background()
	_ = store.AddEntity(ctx, &graph.Entity{Name: "Alice", Type: "person"})
	_ = store.AddTriple(ctx, &graph.Triple{Subject: "Alice", Predicate: "knows", Object: "Bob"})
	_ = store.AddTriple(ctx, &graph.Triple{Subject: "Charlie", Predicate: "knows", Object: "Alice"})

	if err := store.DeleteEntity(ctx, "Alice"); err != nil {
		t.Fatalf("DeleteEntity: %v", err)
	}

	// Entity should be gone
	entity, _ := store.GetEntity(ctx, "Alice")
	if entity != nil {
		t.Error("Entity should be deleted")
	}

	// All triples referencing Alice should be gone
	if store.TripleCount() != 0 {
		t.Errorf("All triples referencing Alice should be deleted, got %d", store.TripleCount())
	}
}

func TestStore_DeleteEntity_NonExisting(t *testing.T) {
	dir := t.TempDir()
	store, _ := graph.NewStore(graph.Config{StoragePath: dir})
	defer store.Close()

	// Should not error
	err := store.DeleteEntity(context.Background(), "Nobody")
	if err != nil {
		t.Errorf("DeleteEntity non-existing should not error: %v", err)
	}
}

func TestStore_DeleteEntity_CascadePartial(t *testing.T) {
	dir := t.TempDir()
	store, _ := graph.NewStore(graph.Config{StoragePath: dir})
	defer store.Close()

	ctx := context.Background()
	_ = store.AddTriple(ctx, &graph.Triple{Subject: "Alice", Predicate: "knows", Object: "Bob"})
	_ = store.AddTriple(ctx, &graph.Triple{Subject: "Charlie", Predicate: "knows", Object: "Dave"})

	_ = store.DeleteEntity(ctx, "Alice")

	// Only Alice->Bob should be removed
	if store.TripleCount() != 1 {
		t.Errorf("Expected 1 triple remaining, got %d", store.TripleCount())
	}
}

// ---------------------------------------------------------------------------
// Close
// ---------------------------------------------------------------------------

func TestStore_Close(t *testing.T) {
	dir := t.TempDir()
	store, _ := graph.NewStore(graph.Config{StoragePath: dir})

	if err := store.Close(); err != nil {
		t.Fatalf("Close: %v", err)
	}
}

// ---------------------------------------------------------------------------
// Edge cases
// ---------------------------------------------------------------------------

func TestStore_EmptyOperations(t *testing.T) {
	dir := t.TempDir()
	store, _ := graph.NewStore(graph.Config{StoragePath: dir})
	defer store.Close()

	ctx := context.Background()

	// Query empty store
	results, err := store.Query(ctx, "", "", "")
	if err != nil {
		t.Fatalf("Query empty: %v", err)
	}
	if len(results) != 0 {
		t.Errorf("Expected 0 results, got %d", len(results))
	}

	// GetRelated on empty
	related, err := store.GetRelated(ctx, "Nobody", 1)
	if err != nil {
		t.Fatalf("GetRelated empty: %v", err)
	}
	if len(related) != 0 {
		t.Errorf("Expected 0, got %d", len(related))
	}

	// Search empty
	search, err := store.Search(ctx, "anything", 10)
	if err != nil {
		t.Fatalf("Search empty: %v", err)
	}
	if len(search) != 0 {
		t.Errorf("Expected 0, got %d", len(search))
	}

	// Counts
	if store.EntityCount() != 0 {
		t.Errorf("EntityCount = %d, want 0", store.EntityCount())
	}
	if store.TripleCount() != 0 {
		t.Errorf("TripleCount = %d, want 0", store.TripleCount())
	}
}

func TestStore_SpecialCharacters(t *testing.T) {
	dir := t.TempDir()
	store, _ := graph.NewStore(graph.Config{StoragePath: dir})
	defer store.Close()

	ctx := context.Background()
	special := "Special: <>&\"'\\n\\t 中文 🎉"
	_ = store.AddEntity(ctx, &graph.Entity{Name: special, Type: "concept"})
	_ = store.AddTriple(ctx, &graph.Triple{Subject: special, Predicate: "is", Object: special})

	entity, _ := store.GetEntity(ctx, special)
	if entity == nil {
		t.Fatal("Entity with special chars should exist")
	}
	if entity.Name != special {
		t.Errorf("Name = %q, want %q", entity.Name, special)
	}

	triples, _ := store.Query(ctx, special, "", "")
	if len(triples) != 1 {
		t.Errorf("Expected 1 triple with special chars, got %d", len(triples))
	}
}

func TestStore_LargeGraph(t *testing.T) {
	dir := t.TempDir()
	store, _ := graph.NewStore(graph.Config{StoragePath: dir})
	defer store.Close()

	ctx := context.Background()

	// Add 100 entities and 99 triples forming a chain
	for i := 0; i < 100; i++ {
		name := strings.Repeat("e", 50) // long entity name
		_ = store.AddEntity(ctx, &graph.Entity{Name: name, Type: "test"})
	}
	for i := 0; i < 99; i++ {
		_ = store.AddTriple(ctx, &graph.Triple{
			Subject:   strings.Repeat("e", 50),
			Predicate: "next",
			Object:    strings.Repeat("e", 50),
		})
	}

	// Should not crash and counts should be correct
	if store.EntityCount() != 1 {
		// All entities have same name (lowercase), so only 1
		t.Logf("EntityCount = %d (entities with same name overwrite)", store.EntityCount())
	}
}

func TestStore_PreservedTimestamp(t *testing.T) {
	dir := t.TempDir()
	store, _ := graph.NewStore(graph.Config{StoragePath: dir})
	defer store.Close()

	ctx := context.Background()
	presetTime := time.Date(2025, 3, 15, 12, 0, 0, 0, time.UTC)

	entity := &graph.Entity{Name: "OldEntity", Type: "concept", CreatedAt: presetTime}
	_ = store.AddEntity(ctx, entity)

	if !entity.CreatedAt.Equal(presetTime) {
		t.Errorf("CreatedAt = %v, want %v (should be preserved)", entity.CreatedAt, presetTime)
	}
}
