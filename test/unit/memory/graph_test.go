package memory_test

import (
	"context"
	"testing"

	graph "github.com/276793422/NemesisBot/module/memory/graph"
)

// newTestGraphStore creates a fresh graph store backed by a temp directory.
func newTestGraphStore(t *testing.T) *graph.Store {
	t.Helper()
	cfg := graph.Config{StoragePath: t.TempDir()}
	s, err := graph.NewStore(cfg)
	if err != nil {
		t.Fatalf("NewStore: %v", err)
	}
	t.Cleanup(func() { s.Close() })
	return s
}

func TestGraphStore_AddAndGetEntity(t *testing.T) {
	s := newTestGraphStore(t)
	ctx := context.Background()

	entity := &graph.Entity{
		Name:       "Alice",
		Type:       "person",
		Properties: map[string]string{"age": "30", "city": "Tokyo"},
	}

	if err := s.AddEntity(ctx, entity); err != nil {
		t.Fatalf("AddEntity: %v", err)
	}

	got, err := s.GetEntity(ctx, "Alice")
	if err != nil {
		t.Fatalf("GetEntity: %v", err)
	}
	if got == nil {
		t.Fatal("GetEntity returned nil")
	}
	if got.Name != "Alice" {
		t.Errorf("Name: got %q, want %q", got.Name, "Alice")
	}
	if got.Type != "person" {
		t.Errorf("Type: got %q, want %q", got.Type, "person")
	}
	if got.Properties["city"] != "Tokyo" {
		t.Errorf("Properties[city]: got %q", got.Properties["city"])
	}

	// Case-insensitive retrieval.
	got2, err := s.GetEntity(ctx, "alice")
	if err != nil {
		t.Fatalf("GetEntity case-insensitive: %v", err)
	}
	if got2 == nil {
		t.Error("expected case-insensitive lookup to work")
	}

	// Non-existent entity.
	got3, err := s.GetEntity(ctx, "NonExistent")
	if err != nil {
		t.Fatalf("GetEntity non-existent: %v", err)
	}
	if got3 != nil {
		t.Error("expected nil for non-existent entity")
	}
}

func TestGraphStore_AddEntityErrors(t *testing.T) {
	s := newTestGraphStore(t)
	ctx := context.Background()

	if err := s.AddEntity(ctx, nil); err == nil {
		t.Error("expected error for nil entity")
	}
	if err := s.AddEntity(ctx, &graph.Entity{Name: ""}); err == nil {
		t.Error("expected error for empty name")
	}
}

func TestGraphStore_AddTriple(t *testing.T) {
	s := newTestGraphStore(t)
	ctx := context.Background()

	triple := &graph.Triple{
		Subject:    "Alice",
		Predicate:  "knows",
		Object:     "Bob",
		Confidence: 0.95,
	}

	if err := s.AddTriple(ctx, triple); err != nil {
		t.Fatalf("AddTriple: %v", err)
	}

	if s.TripleCount() != 1 {
		t.Errorf("expected 1 triple, got %d", s.TripleCount())
	}
}

func TestGraphStore_AddTripleErrors(t *testing.T) {
	s := newTestGraphStore(t)
	ctx := context.Background()

	if err := s.AddTriple(ctx, nil); err == nil {
		t.Error("expected error for nil triple")
	}
	if err := s.AddTriple(ctx, &graph.Triple{Subject: "A", Predicate: "B"}); err == nil {
		t.Error("expected error for missing object")
	}
	if err := s.AddTriple(ctx, &graph.Triple{Subject: "A", Object: "C"}); err == nil {
		t.Error("expected error for missing predicate")
	}
}

func TestGraphStore_AddTripleDuplicate(t *testing.T) {
	s := newTestGraphStore(t)
	ctx := context.Background()

	t1 := &graph.Triple{Subject: "A", Predicate: "knows", Object: "B", Confidence: 0.5}
	t2 := &graph.Triple{Subject: "A", Predicate: "knows", Object: "B", Confidence: 0.9}

	_ = s.AddTriple(ctx, t1)
	_ = s.AddTriple(ctx, t2)

	// Duplicate should update, not add.
	if s.TripleCount() != 1 {
		t.Errorf("expected 1 triple after duplicate add, got %d", s.TripleCount())
	}

	// Verify the confidence was updated.
	triples, _ := s.Query(ctx, "A", "knows", "B")
	if len(triples) != 1 {
		t.Fatalf("expected 1 result, got %d", len(triples))
	}
	if triples[0].Confidence != 0.9 {
		t.Errorf("expected confidence 0.9, got %f", triples[0].Confidence)
	}
}

func TestGraphStore_QueryBySubject(t *testing.T) {
	s := newTestGraphStore(t)
	ctx := context.Background()

	_ = s.AddTriple(ctx, &graph.Triple{Subject: "Alice", Predicate: "knows", Object: "Bob"})
	_ = s.AddTriple(ctx, &graph.Triple{Subject: "Alice", Predicate: "works_at", Object: "Acme"})
	_ = s.AddTriple(ctx, &graph.Triple{Subject: "Bob", Predicate: "knows", Object: "Carol"})

	results, err := s.Query(ctx, "Alice", "", "")
	if err != nil {
		t.Fatalf("Query: %v", err)
	}
	if len(results) != 2 {
		t.Errorf("expected 2 results for subject=Alice, got %d", len(results))
	}
}

func TestGraphStore_QueryByPredicate(t *testing.T) {
	s := newTestGraphStore(t)
	ctx := context.Background()

	_ = s.AddTriple(ctx, &graph.Triple{Subject: "Alice", Predicate: "knows", Object: "Bob"})
	_ = s.AddTriple(ctx, &graph.Triple{Subject: "Bob", Predicate: "knows", Object: "Carol"})
	_ = s.AddTriple(ctx, &graph.Triple{Subject: "Alice", Predicate: "works_at", Object: "Acme"})

	results, err := s.Query(ctx, "", "knows", "")
	if err != nil {
		t.Fatalf("Query: %v", err)
	}
	if len(results) != 2 {
		t.Errorf("expected 2 results for predicate=knows, got %d", len(results))
	}
}

func TestGraphStore_QueryWildcard(t *testing.T) {
	s := newTestGraphStore(t)
	ctx := context.Background()

	_ = s.AddTriple(ctx, &graph.Triple{Subject: "A", Predicate: "x", Object: "B"})
	_ = s.AddTriple(ctx, &graph.Triple{Subject: "B", Predicate: "y", Object: "C"})

	// All empty = wildcard, returns all.
	results, err := s.Query(ctx, "", "", "")
	if err != nil {
		t.Fatalf("Query wildcard: %v", err)
	}
	if len(results) != 2 {
		t.Errorf("expected 2 results for wildcard query, got %d", len(results))
	}
}

func TestGraphStore_QueryByObject(t *testing.T) {
	s := newTestGraphStore(t)
	ctx := context.Background()

	_ = s.AddTriple(ctx, &graph.Triple{Subject: "Alice", Predicate: "knows", Object: "Bob"})
	_ = s.AddTriple(ctx, &graph.Triple{Subject: "Carol", Predicate: "knows", Object: "Bob"})
	_ = s.AddTriple(ctx, &graph.Triple{Subject: "Dave", Predicate: "knows", Object: "Eve"})

	results, err := s.Query(ctx, "", "", "Bob")
	if err != nil {
		t.Fatalf("Query: %v", err)
	}
	if len(results) != 2 {
		t.Errorf("expected 2 results for object=Bob, got %d", len(results))
	}
}

func TestGraphStore_GetRelated(t *testing.T) {
	s := newTestGraphStore(t)
	ctx := context.Background()

	// Build a small graph: Alice --knows--> Bob --knows--> Carol
	_ = s.AddTriple(ctx, &graph.Triple{Subject: "Alice", Predicate: "knows", Object: "Bob"})
	_ = s.AddTriple(ctx, &graph.Triple{Subject: "Bob", Predicate: "knows", Object: "Carol"})

	// Depth 1: only direct relationships from Alice.
	// The BFS starts at "alice", finds Alice-knows->Bob (subject matches),
	// so neighbor is "bob". One triple returned.
	d1, err := s.GetRelated(ctx, "Alice", 1)
	if err != nil {
		t.Fatalf("GetRelated depth=1: %v", err)
	}
	if len(d1) < 1 {
		t.Errorf("expected at least 1 triple at depth 1, got %d", len(d1))
	}

	// Depth 2: BFS explores alice (finds Alice->Bob, neighbor=bob),
	// then explores bob (finds Alice->Bob since Object=bob AND Bob->Carol since Subject=bob).
	// So we get at least Alice->Bob and Bob->Carol.
	d2, err := s.GetRelated(ctx, "Alice", 2)
	if err != nil {
		t.Fatalf("GetRelated depth=2: %v", err)
	}
	if len(d2) < 2 {
		t.Errorf("expected at least 2 triples at depth 2, got %d", len(d2))
	}

	// Verify Bob->Carol is among the results.
	foundBobCarol := false
	for _, tr := range d2 {
		if tr.Subject == "Bob" && tr.Predicate == "knows" && tr.Object == "Carol" {
			foundBobCarol = true
			break
		}
	}
	if !foundBobCarol {
		t.Error("expected Bob->Carol triple in depth=2 results")
	}
}

func TestGraphStore_DeleteEntity(t *testing.T) {
	s := newTestGraphStore(t)
	ctx := context.Background()

	_ = s.AddEntity(ctx, &graph.Entity{Name: "Alice", Type: "person"})
	_ = s.AddEntity(ctx, &graph.Entity{Name: "Bob", Type: "person"})
	_ = s.AddTriple(ctx, &graph.Triple{Subject: "Alice", Predicate: "knows", Object: "Bob"})
	_ = s.AddTriple(ctx, &graph.Triple{Subject: "Bob", Predicate: "works_at", Object: "Acme"})

	if err := s.DeleteEntity(ctx, "Alice"); err != nil {
		t.Fatalf("DeleteEntity: %v", err)
	}

	// Entity should be gone.
	got, err := s.GetEntity(ctx, "Alice")
	if err != nil {
		t.Fatalf("GetEntity: %v", err)
	}
	if got != nil {
		t.Error("expected nil after entity deletion")
	}

	// Triples referencing Alice should be removed.
	remaining, _ := s.Query(ctx, "", "", "")
	for _, tr := range remaining {
		if tr.Subject == "Alice" || tr.Object == "Alice" {
			t.Errorf("found triple still referencing Alice: %s --[%s]--> %s", tr.Subject, tr.Predicate, tr.Object)
		}
	}

	// Bob's triple to Acme should remain.
	bobTriples, _ := s.Query(ctx, "Bob", "", "")
	if len(bobTriples) != 1 {
		t.Errorf("expected 1 remaining triple for Bob, got %d", len(bobTriples))
	}
}

func TestGraphStore_Search(t *testing.T) {
	s := newTestGraphStore(t)
	ctx := context.Background()

	_ = s.AddTriple(ctx, &graph.Triple{Subject: "Alice", Predicate: "lives_in", Object: "Tokyo"})
	_ = s.AddTriple(ctx, &graph.Triple{Subject: "Bob", Predicate: "lives_in", Object: "Paris"})
	_ = s.AddTriple(ctx, &graph.Triple{Subject: "Carol", Predicate: "works_at", Object: "Acme"})

	// Search for "lives_in" in predicate.
	results, err := s.Search(ctx, "lives_in", 10)
	if err != nil {
		t.Fatalf("Search: %v", err)
	}
	if len(results) != 2 {
		t.Errorf("expected 2 results for 'lives_in', got %d", len(results))
	}

	// Search for "tokyo" in object.
	results2, err := s.Search(ctx, "tokyo", 10)
	if err != nil {
		t.Fatalf("Search: %v", err)
	}
	if len(results2) != 1 {
		t.Errorf("expected 1 result for 'tokyo', got %d", len(results2))
	}

	// Search with limit.
	results3, err := s.Search(ctx, "lives_in", 1)
	if err != nil {
		t.Fatalf("Search: %v", err)
	}
	if len(results3) != 1 {
		t.Errorf("expected 1 result with limit=1, got %d", len(results3))
	}
}

func TestGraphStore_Persistence(t *testing.T) {
	tmp := t.TempDir()

	cfg := graph.Config{StoragePath: tmp}
	s1, err := graph.NewStore(cfg)
	if err != nil {
		t.Fatalf("NewStore: %v", err)
	}
	ctx := context.Background()
	_ = s1.AddEntity(ctx, &graph.Entity{Name: "Alice", Type: "person"})
	_ = s1.AddTriple(ctx, &graph.Triple{Subject: "Alice", Predicate: "knows", Object: "Bob"})
	s1.Close()

	s2, err := graph.NewStore(cfg)
	if err != nil {
		t.Fatalf("NewStore reload: %v", err)
	}
	defer s2.Close()

	got, err := s2.GetEntity(ctx, "Alice")
	if err != nil {
		t.Fatalf("GetEntity: %v", err)
	}
	if got == nil || got.Name != "Alice" {
		t.Error("entity did not survive persistence")
	}

	triples, err := s2.Query(ctx, "Alice", "", "")
	if err != nil {
		t.Fatalf("Query: %v", err)
	}
	if len(triples) != 1 {
		t.Errorf("expected 1 triple after reload, got %d", len(triples))
	}
}

func TestGraphStore_EntityAndTripleCount(t *testing.T) {
	s := newTestGraphStore(t)
	ctx := context.Background()

	if s.EntityCount() != 0 || s.TripleCount() != 0 {
		t.Fatal("expected initial counts to be 0")
	}

	_ = s.AddEntity(ctx, &graph.Entity{Name: "A", Type: "person"})
	_ = s.AddEntity(ctx, &graph.Entity{Name: "B", Type: "person"})
	_ = s.AddTriple(ctx, &graph.Triple{Subject: "A", Predicate: "knows", Object: "B"})

	if s.EntityCount() != 2 {
		t.Errorf("expected 2 entities, got %d", s.EntityCount())
	}
	if s.TripleCount() != 1 {
		t.Errorf("expected 1 triple, got %d", s.TripleCount())
	}
}

func TestGraphStore_QueryNoMatch(t *testing.T) {
	s := newTestGraphStore(t)
	ctx := context.Background()

	_ = s.AddTriple(ctx, &graph.Triple{Subject: "A", Predicate: "x", Object: "B"})

	results, err := s.Query(ctx, "Z", "", "")
	if err != nil {
		t.Fatalf("Query: %v", err)
	}
	if len(results) != 0 {
		t.Errorf("expected 0 results for non-matching subject, got %d", len(results))
	}
}
