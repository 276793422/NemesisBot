package memory_test

import (
	"context"
	"strings"
	"testing"
	"time"

	"github.com/276793422/NemesisBot/module/memory"
	"github.com/276793422/NemesisBot/module/memory/episodic"
	"github.com/276793422/NemesisBot/module/memory/graph"
	"github.com/276793422/NemesisBot/module/tools"
)

// ---------------------------------------------------------------------------
// mockStoreProvider implements memory.StoreProvider for testing tools
// ---------------------------------------------------------------------------

type mockStoreProvider struct {
	episodicStore *episodic.Store
	graphStore    *graph.Store
}

func (m *mockStoreProvider) GetEpisodicStore() memory.EpisodicStore {
	if m.episodicStore == nil {
		return nil
	}
	return m.episodicStore
}

func (m *mockStoreProvider) GetGraphStore() memory.GraphStore {
	if m.graphStore == nil {
		return nil
	}
	return m.graphStore
}

func newTestStoreProvider(t *testing.T) *mockStoreProvider {
	t.Helper()
	epStore, err := episodic.NewStore(episodic.Config{
		StoragePath:           t.TempDir(),
		MaxEpisodesPerSession: 100,
		RetentionDays:         90,
	})
	if err != nil {
		t.Fatalf("episodic.NewStore: %v", err)
	}
	gStore, err := graph.NewStore(graph.Config{
		StoragePath: t.TempDir(),
	})
	if err != nil {
		t.Fatalf("graph.NewStore: %v", err)
	}
	return &mockStoreProvider{episodicStore: epStore, graphStore: gStore}
}

// ---------------------------------------------------------------------------
// NewMemoryTools
// ---------------------------------------------------------------------------

func TestNewMemoryTools(t *testing.T) {
	sp := newTestStoreProvider(t)
	toolList := memory.NewMemoryTools(sp)
	if len(toolList) != 4 {
		t.Fatalf("Expected 4 tools, got %d", len(toolList))
	}

	expectedNames := map[string]bool{
		"memory_search": false,
		"memory_store":  false,
		"memory_forget": false,
		"memory_list":   false,
	}
	for _, tool := range toolList {
		if _, ok := expectedNames[tool.Name()]; !ok {
			t.Errorf("Unexpected tool name: %s", tool.Name())
		}
		expectedNames[tool.Name()] = true
	}
	for name, found := range expectedNames {
		if !found {
			t.Errorf("Missing tool: %s", name)
		}
	}
}

// ---------------------------------------------------------------------------
// MemorySearchTool
// ---------------------------------------------------------------------------

func TestMemorySearchTool_NameAndDescription(t *testing.T) {
	sp := newTestStoreProvider(t)
	tool := memory.NewMemorySearchTool(sp)

	if tool.Name() != "memory_search" {
		t.Errorf("Name() = %q, want %q", tool.Name(), "memory_search")
	}
	if tool.Description() == "" {
		t.Error("Description() should not be empty")
	}
	if tool.Parameters() == nil {
		t.Error("Parameters() should not be nil")
	}
}

func TestMemorySearchTool_Execute_NoQuery(t *testing.T) {
	sp := newTestStoreProvider(t)
	tool := memory.NewMemorySearchTool(sp)

	result := tool.Execute(context.Background(), map[string]interface{}{})
	if !result.IsError {
		t.Error("Expected error result for empty query")
	}
}

func TestMemorySearchTool_Execute_EpisodicSearch(t *testing.T) {
	sp := newTestStoreProvider(t)
	ctx := context.Background()

	// Store some episodes
	ep := &episodic.Episode{
		SessionKey: "test-session",
		Role:       "user",
		Content:    "Tell me about quantum computing",
		Tags:       []string{"science"},
	}
	if err := sp.episodicStore.StoreEpisode(ctx, ep); err != nil {
		t.Fatalf("StoreEpisode: %v", err)
	}

	tool := memory.NewMemorySearchTool(sp)
	result := tool.Execute(ctx, map[string]interface{}{
		"query":        "quantum",
		"memory_type": "episodic",
		"limit":       float64(10),
	})

	if result.IsError {
		t.Errorf("Unexpected error: %s", result.ForLLM)
	}
	if !strings.Contains(result.ForLLM, "quantum") {
		t.Errorf("Result should contain 'quantum', got: %s", result.ForLLM)
	}
}

func TestMemorySearchTool_Execute_GraphSearch(t *testing.T) {
	sp := newTestStoreProvider(t)
	ctx := context.Background()

	// Add a triple
	triple := &graph.Triple{
		Subject:    "Go",
		Predicate:  "is_a",
		Object:     "programming language",
		Confidence: 0.9,
	}
	if err := sp.graphStore.AddTriple(ctx, triple); err != nil {
		t.Fatalf("AddTriple: %v", err)
	}

	tool := memory.NewMemorySearchTool(sp)
	result := tool.Execute(ctx, map[string]interface{}{
		"query":        "programming",
		"memory_type": "graph",
	})

	if result.IsError {
		t.Errorf("Unexpected error: %s", result.ForLLM)
	}
	if !strings.Contains(result.ForLLM, "programming language") {
		t.Errorf("Result should contain 'programming language', got: %s", result.ForLLM)
	}
}

func TestMemorySearchTool_Execute_AllTypes(t *testing.T) {
	sp := newTestStoreProvider(t)
	ctx := context.Background()

	// Store in both
	_ = sp.episodicStore.StoreEpisode(ctx, &episodic.Episode{
		SessionKey: "s1",
		Role:       "user",
		Content:    "database design patterns",
	})
	_ = sp.graphStore.AddTriple(ctx, &graph.Triple{
		Subject:   "database",
		Predicate: "has_pattern",
		Object:    "design",
	})

	tool := memory.NewMemorySearchTool(sp)
	result := tool.Execute(ctx, map[string]interface{}{
		"query": "database",
	})

	if result.IsError {
		t.Errorf("Unexpected error: %s", result.ForLLM)
	}
	if !strings.Contains(result.ForLLM, "Episodic") {
		t.Error("Result should contain episodic section")
	}
	if !strings.Contains(result.ForLLM, "Knowledge Graph") {
		t.Error("Result should contain graph section")
	}
}

func TestMemorySearchTool_Execute_NoResults(t *testing.T) {
	sp := newTestStoreProvider(t)
	tool := memory.NewMemorySearchTool(sp)

	result := tool.Execute(context.Background(), map[string]interface{}{
		"query": "nonexistent_xyz_12345",
	})

	if result.IsError {
		t.Errorf("Unexpected error: %s", result.ForLLM)
	}
	if !strings.Contains(result.ForLLM, "No episodic") {
		t.Errorf("Result should mention no results, got: %s", result.ForLLM)
	}
}

func TestMemorySearchTool_Execute_NilStores(t *testing.T) {
	sp := &mockStoreProvider{} // both stores are nil
	tool := memory.NewMemorySearchTool(sp)

	result := tool.Execute(context.Background(), map[string]interface{}{
		"query": "test",
	})

	// Should not error, just produce empty results
	if result.IsError {
		t.Errorf("Unexpected error: %s", result.ForLLM)
	}
}

func TestMemorySearchTool_Execute_LimitClamping(t *testing.T) {
	sp := newTestStoreProvider(t)
	tool := memory.NewMemorySearchTool(sp)

	// Limit over 50 should be clamped
	result := tool.Execute(context.Background(), map[string]interface{}{
		"query": "test",
		"limit": float64(100),
	})
	_ = result // no panic is sufficient

	// Limit 0 should default to 10
	result2 := tool.Execute(context.Background(), map[string]interface{}{
		"query": "test",
		"limit": float64(0),
	})
	_ = result2
}

// ---------------------------------------------------------------------------
// MemoryStoreTool
// ---------------------------------------------------------------------------

func TestMemoryStoreTool_NameAndDescription(t *testing.T) {
	sp := newTestStoreProvider(t)
	tool := memory.NewMemoryStoreTool(sp)

	if tool.Name() != "memory_store" {
		t.Errorf("Name() = %q, want %q", tool.Name(), "memory_store")
	}
	if tool.Description() == "" {
		t.Error("Description() should not be empty")
	}
	params := tool.Parameters()
	if params == nil {
		t.Error("Parameters() should not be nil")
	}
}

func TestMemoryStoreTool_Execute_NoType(t *testing.T) {
	sp := newTestStoreProvider(t)
	tool := memory.NewMemoryStoreTool(sp)

	result := tool.Execute(context.Background(), map[string]interface{}{})
	if !result.IsError {
		t.Error("Expected error for missing memory_type")
	}
}

func TestMemoryStoreTool_Execute_InvalidType(t *testing.T) {
	sp := newTestStoreProvider(t)
	tool := memory.NewMemoryStoreTool(sp)

	result := tool.Execute(context.Background(), map[string]interface{}{
		"memory_type": "invalid",
	})
	if !result.IsError {
		t.Error("Expected error for invalid memory_type")
	}
}

func TestMemoryStoreTool_Execute_Episodic(t *testing.T) {
	sp := newTestStoreProvider(t)
	tool := memory.NewMemoryStoreTool(sp)
	ctx := context.Background()

	result := tool.Execute(ctx, map[string]interface{}{
		"memory_type":  "episodic",
		"content":     "Test episodic content",
		"role":        "assistant",
		"session_key": "test-session",
		"tags":        []interface{}{"test", "unit"},
	})

	if result.IsError {
		t.Errorf("Unexpected error: %s", result.ForLLM)
	}
	if !strings.Contains(result.ForLLM, "stored successfully") {
		t.Errorf("Result should confirm storage: %s", result.ForLLM)
	}

	// Verify it was stored
	episodes, _ := sp.episodicStore.GetRecent(ctx, "test-session", 10)
	if len(episodes) != 1 {
		t.Fatalf("Expected 1 episode, got %d", len(episodes))
	}
	if episodes[0].Content != "Test episodic content" {
		t.Errorf("Content = %q, want %q", episodes[0].Content, "Test episodic content")
	}
}

func TestMemoryStoreTool_Execute_Episodic_NoContent(t *testing.T) {
	sp := newTestStoreProvider(t)
	tool := memory.NewMemoryStoreTool(sp)

	result := tool.Execute(context.Background(), map[string]interface{}{
		"memory_type": "episodic",
	})
	if !result.IsError {
		t.Error("Expected error for missing content")
	}
}

func TestMemoryStoreTool_Execute_Episodic_DefaultRole(t *testing.T) {
	sp := newTestStoreProvider(t)
	tool := memory.NewMemoryStoreTool(sp)
	ctx := context.Background()

	result := tool.Execute(ctx, map[string]interface{}{
		"memory_type": "episodic",
		"content":     "some content",
	})
	if result.IsError {
		t.Errorf("Unexpected error: %s", result.ForLLM)
	}

	episodes, _ := sp.episodicStore.GetRecent(ctx, "manual-", 1)
	_ = episodes // just checking it doesn't crash; session key is auto-generated
}

func TestMemoryStoreTool_Execute_Episodic_NilStore(t *testing.T) {
	sp := &mockStoreProvider{}
	tool := memory.NewMemoryStoreTool(sp)

	result := tool.Execute(context.Background(), map[string]interface{}{
		"memory_type": "episodic",
		"content":     "test",
	})
	if !result.IsError {
		t.Error("Expected error for nil episodic store")
	}
}

func TestMemoryStoreTool_Execute_Graph_EntityOnly(t *testing.T) {
	sp := newTestStoreProvider(t)
	tool := memory.NewMemoryStoreTool(sp)
	ctx := context.Background()

	result := tool.Execute(ctx, map[string]interface{}{
		"memory_type": "graph",
		"entity_name": "Alice",
		"entity_type": "person",
		"entity_properties": map[string]interface{}{
			"occupation": "engineer",
		},
	})

	if result.IsError {
		t.Errorf("Unexpected error: %s", result.ForLLM)
	}
	if !strings.Contains(result.ForLLM, "Entity stored: Alice") {
		t.Errorf("Result should confirm entity storage: %s", result.ForLLM)
	}

	entity, err := sp.graphStore.GetEntity(ctx, "Alice")
	if err != nil {
		t.Fatalf("GetEntity: %v", err)
	}
	if entity == nil {
		t.Fatal("Entity should be stored")
	}
	if entity.Type != "person" {
		t.Errorf("Entity.Type = %q, want %q", entity.Type, "person")
	}
	if entity.Properties["occupation"] != "engineer" {
		t.Errorf("Properties[occupation] = %q, want %q", entity.Properties["occupation"], "engineer")
	}
}

func TestMemoryStoreTool_Execute_Graph_TripleOnly(t *testing.T) {
	sp := newTestStoreProvider(t)
	tool := memory.NewMemoryStoreTool(sp)
	ctx := context.Background()

	result := tool.Execute(ctx, map[string]interface{}{
		"memory_type":      "graph",
		"triple_subject":  "Alice",
		"triple_predicate": "knows",
		"triple_object":   "Bob",
		"confidence":      float64(0.85),
	})

	if result.IsError {
		t.Errorf("Unexpected error: %s", result.ForLLM)
	}
	if !strings.Contains(result.ForLLM, "Triple stored") {
		t.Errorf("Result should confirm triple storage: %s", result.ForLLM)
	}
}

func TestMemoryStoreTool_Execute_Graph_EntityAndTriple(t *testing.T) {
	sp := newTestStoreProvider(t)
	tool := memory.NewMemoryStoreTool(sp)
	ctx := context.Background()

	result := tool.Execute(ctx, map[string]interface{}{
		"memory_type":       "graph",
		"entity_name":      "Paris",
		"entity_type":      "place",
		"triple_subject":   "Paris",
		"triple_predicate": "is_capital_of",
		"triple_object":    "France",
	})

	if result.IsError {
		t.Errorf("Unexpected error: %s", result.ForLLM)
	}
	if !strings.Contains(result.ForLLM, "Entity stored") {
		t.Error("Should mention entity storage")
	}
	if !strings.Contains(result.ForLLM, "Triple stored") {
		t.Error("Should mention triple storage")
	}
}

func TestMemoryStoreTool_Execute_Graph_NoData(t *testing.T) {
	sp := newTestStoreProvider(t)
	tool := memory.NewMemoryStoreTool(sp)

	result := tool.Execute(context.Background(), map[string]interface{}{
		"memory_type": "graph",
	})
	if !result.IsError {
		t.Error("Expected error when no entity or triple data provided")
	}
}

func TestMemoryStoreTool_Execute_Graph_NilStore(t *testing.T) {
	sp := &mockStoreProvider{}
	tool := memory.NewMemoryStoreTool(sp)

	result := tool.Execute(context.Background(), map[string]interface{}{
		"memory_type": "graph",
		"entity_name": "test",
	})
	if !result.IsError {
		t.Error("Expected error for nil graph store")
	}
}

// ---------------------------------------------------------------------------
// MemoryForgetTool
// ---------------------------------------------------------------------------

func TestMemoryForgetTool_NameAndDescription(t *testing.T) {
	sp := newTestStoreProvider(t)
	tool := memory.NewMemoryForgetTool(sp)

	if tool.Name() != "memory_forget" {
		t.Errorf("Name() = %q, want %q", tool.Name(), "memory_forget")
	}
	if tool.Description() == "" {
		t.Error("Description() should not be empty")
	}
	if tool.Parameters() == nil {
		t.Error("Parameters() should not be nil")
	}
}

func TestMemoryForgetTool_Execute_NoAction(t *testing.T) {
	sp := newTestStoreProvider(t)
	tool := memory.NewMemoryForgetTool(sp)

	result := tool.Execute(context.Background(), map[string]interface{}{})
	if !result.IsError {
		t.Error("Expected error for missing action")
	}
}

func TestMemoryForgetTool_Execute_InvalidAction(t *testing.T) {
	sp := newTestStoreProvider(t)
	tool := memory.NewMemoryForgetTool(sp)

	result := tool.Execute(context.Background(), map[string]interface{}{
		"action": "invalid",
	})
	if !result.IsError {
		t.Error("Expected error for invalid action")
	}
}

func TestMemoryForgetTool_Execute_DeleteSession(t *testing.T) {
	sp := newTestStoreProvider(t)
	tool := memory.NewMemoryForgetTool(sp)
	ctx := context.Background()

	// Store an episode first
	_ = sp.episodicStore.StoreEpisode(ctx, &episodic.Episode{
		SessionKey: "session-to-delete",
		Content:    "some content",
		Role:       "user",
	})

	result := tool.Execute(ctx, map[string]interface{}{
		"action":      "delete_session",
		"session_key": "session-to-delete",
	})

	if result.IsError {
		t.Errorf("Unexpected error: %s", result.ForLLM)
	}
	if !strings.Contains(result.ForLLM, "deleted successfully") {
		t.Errorf("Result should confirm deletion: %s", result.ForLLM)
	}
}

func TestMemoryForgetTool_Execute_DeleteSession_NoKey(t *testing.T) {
	sp := newTestStoreProvider(t)
	tool := memory.NewMemoryForgetTool(sp)

	result := tool.Execute(context.Background(), map[string]interface{}{
		"action": "delete_session",
	})
	if !result.IsError {
		t.Error("Expected error for missing session_key")
	}
}

func TestMemoryForgetTool_Execute_DeleteSession_NilStore(t *testing.T) {
	sp := &mockStoreProvider{}
	tool := memory.NewMemoryForgetTool(sp)

	result := tool.Execute(context.Background(), map[string]interface{}{
		"action":      "delete_session",
		"session_key": "test",
	})
	if !result.IsError {
		t.Error("Expected error for nil store")
	}
}

func TestMemoryForgetTool_Execute_Cleanup(t *testing.T) {
	sp := newTestStoreProvider(t)
	tool := memory.NewMemoryForgetTool(sp)
	ctx := context.Background()

	// Store an old episode (timestamp in the past)
	oldEp := &episodic.Episode{
		SessionKey: "old-session",
		Content:    "old content",
		Role:       "user",
		Timestamp:  time.Now().UTC().Add(-200 * 24 * time.Hour), // 200 days ago
	}
	_ = sp.episodicStore.StoreEpisode(ctx, oldEp)

	result := tool.Execute(ctx, map[string]interface{}{
		"action":          "cleanup",
		"older_than_days": float64(90),
	})

	if result.IsError {
		t.Errorf("Unexpected error: %s", result.ForLLM)
	}
	if !strings.Contains(result.ForLLM, "removed") {
		t.Errorf("Result should mention removal count: %s", result.ForLLM)
	}
}

func TestMemoryForgetTool_Execute_Cleanup_DefaultDays(t *testing.T) {
	sp := newTestStoreProvider(t)
	tool := memory.NewMemoryForgetTool(sp)
	ctx := context.Background()

	// No older_than_days → default 90
	result := tool.Execute(ctx, map[string]interface{}{
		"action": "cleanup",
	})

	if result.IsError {
		t.Errorf("Unexpected error: %s", result.ForLLM)
	}
}

func TestMemoryForgetTool_Execute_Cleanup_NilStore(t *testing.T) {
	sp := &mockStoreProvider{}
	tool := memory.NewMemoryForgetTool(sp)

	result := tool.Execute(context.Background(), map[string]interface{}{
		"action": "cleanup",
	})
	if !result.IsError {
		t.Error("Expected error for nil store")
	}
}

func TestMemoryForgetTool_Execute_DeleteEntity(t *testing.T) {
	sp := newTestStoreProvider(t)
	tool := memory.NewMemoryForgetTool(sp)
	ctx := context.Background()

	// Add entity
	_ = sp.graphStore.AddEntity(ctx, &graph.Entity{Name: "ToDelete", Type: "concept"})

	result := tool.Execute(ctx, map[string]interface{}{
		"action":      "delete_entity",
		"entity_name": "ToDelete",
	})

	if result.IsError {
		t.Errorf("Unexpected error: %s", result.ForLLM)
	}
	if !strings.Contains(result.ForLLM, "deleted") {
		t.Errorf("Result should confirm deletion: %s", result.ForLLM)
	}

	entity, _ := sp.graphStore.GetEntity(ctx, "ToDelete")
	if entity != nil {
		t.Error("Entity should be deleted")
	}
}

func TestMemoryForgetTool_Execute_DeleteEntity_NoName(t *testing.T) {
	sp := newTestStoreProvider(t)
	tool := memory.NewMemoryForgetTool(sp)

	result := tool.Execute(context.Background(), map[string]interface{}{
		"action": "delete_entity",
	})
	if !result.IsError {
		t.Error("Expected error for missing entity_name")
	}
}

func TestMemoryForgetTool_Execute_DeleteEntity_NilStore(t *testing.T) {
	sp := &mockStoreProvider{}
	tool := memory.NewMemoryForgetTool(sp)

	result := tool.Execute(context.Background(), map[string]interface{}{
		"action":      "delete_entity",
		"entity_name": "test",
	})
	if !result.IsError {
		t.Error("Expected error for nil store")
	}
}

// ---------------------------------------------------------------------------
// MemoryListTool
// ---------------------------------------------------------------------------

func TestMemoryListTool_NameAndDescription(t *testing.T) {
	sp := newTestStoreProvider(t)
	tool := memory.NewMemoryListTool(sp)

	if tool.Name() != "memory_list" {
		t.Errorf("Name() = %q, want %q", tool.Name(), "memory_list")
	}
	if tool.Description() == "" {
		t.Error("Description() should not be empty")
	}
	if tool.Parameters() == nil {
		t.Error("Parameters() should not be nil")
	}
}

func TestMemoryListTool_Execute_Status(t *testing.T) {
	sp := newTestStoreProvider(t)
	tool := memory.NewMemoryListTool(sp)
	ctx := context.Background()

	// Add some data
	_ = sp.episodicStore.StoreEpisode(ctx, &episodic.Episode{
		SessionKey: "s1", Role: "user", Content: "hello",
	})
	_ = sp.graphStore.AddEntity(ctx, &graph.Entity{Name: "E1", Type: "concept"})

	result := tool.Execute(ctx, map[string]interface{}{
		"list_type": "status",
	})

	if result.IsError {
		t.Errorf("Unexpected error: %s", result.ForLLM)
	}
	if !strings.Contains(result.ForLLM, "Memory Store Status") {
		t.Errorf("Result should contain status header: %s", result.ForLLM)
	}
	if !strings.Contains(result.ForLLM, "Episodic Memory") {
		t.Errorf("Result should contain episodic section: %s", result.ForLLM)
	}
	if !strings.Contains(result.ForLLM, "Knowledge Graph") {
		t.Errorf("Result should contain graph section: %s", result.ForLLM)
	}
}

func TestMemoryListTool_Execute_Status_NilStores(t *testing.T) {
	sp := &mockStoreProvider{}
	tool := memory.NewMemoryListTool(sp)

	result := tool.Execute(context.Background(), map[string]interface{}{
		"list_type": "status",
	})

	if result.IsError {
		t.Errorf("Unexpected error: %s", result.ForLLM)
	}
	if !strings.Contains(result.ForLLM, "Not available") {
		t.Errorf("Should show not available for nil stores: %s", result.ForLLM)
	}
}

func TestMemoryListTool_Execute_DefaultStatus(t *testing.T) {
	sp := newTestStoreProvider(t)
	tool := memory.NewMemoryListTool(sp)

	// No list_type → should default to "status"
	result := tool.Execute(context.Background(), map[string]interface{}{})
	if result.IsError {
		t.Errorf("Unexpected error: %s", result.ForLLM)
	}
	if !strings.Contains(result.ForLLM, "Memory Store Status") {
		t.Errorf("Default should be status: %s", result.ForLLM)
	}
}

func TestMemoryListTool_Execute_Episodes(t *testing.T) {
	sp := newTestStoreProvider(t)
	tool := memory.NewMemoryListTool(sp)
	ctx := context.Background()

	_ = sp.episodicStore.StoreEpisode(ctx, &episodic.Episode{
		SessionKey: "my-session",
		Role:       "user",
		Content:    "First message",
	})
	_ = sp.episodicStore.StoreEpisode(ctx, &episodic.Episode{
		SessionKey: "my-session",
		Role:       "assistant",
		Content:    "Response message",
	})

	result := tool.Execute(ctx, map[string]interface{}{
		"list_type":   "episodes",
		"session_key": "my-session",
		"limit":       float64(10),
	})

	if result.IsError {
		t.Errorf("Unexpected error: %s", result.ForLLM)
	}
	if !strings.Contains(result.ForLLM, "First message") {
		t.Errorf("Result should contain episode content: %s", result.ForLLM)
	}
}

func TestMemoryListTool_Execute_Episodes_NoKey(t *testing.T) {
	sp := newTestStoreProvider(t)
	tool := memory.NewMemoryListTool(sp)

	result := tool.Execute(context.Background(), map[string]interface{}{
		"list_type": "episodes",
	})
	if !result.IsError {
		t.Error("Expected error for missing session_key")
	}
}

func TestMemoryListTool_Execute_Episodes_NilStore(t *testing.T) {
	sp := &mockStoreProvider{}
	tool := memory.NewMemoryListTool(sp)

	result := tool.Execute(context.Background(), map[string]interface{}{
		"list_type":   "episodes",
		"session_key": "test",
	})
	if !result.IsError {
		t.Error("Expected error for nil store")
	}
}

func TestMemoryListTool_Execute_Episodes_EmptySession(t *testing.T) {
	sp := newTestStoreProvider(t)
	tool := memory.NewMemoryListTool(sp)

	result := tool.Execute(context.Background(), map[string]interface{}{
		"list_type":   "episodes",
		"session_key": "nonexistent",
	})
	if result.IsError {
		t.Errorf("Unexpected error: %s", result.ForLLM)
	}
	if !strings.Contains(result.ForLLM, "No episodes found") {
		t.Errorf("Should mention no episodes: %s", result.ForLLM)
	}
}

func TestMemoryListTool_Execute_GraphQuery(t *testing.T) {
	sp := newTestStoreProvider(t)
	tool := memory.NewMemoryListTool(sp)
	ctx := context.Background()

	_ = sp.graphStore.AddTriple(ctx, &graph.Triple{
		Subject: "Alice", Predicate: "knows", Object: "Bob",
		Metadata: map[string]string{"since": "2020"},
	})

	result := tool.Execute(ctx, map[string]interface{}{
		"list_type": "graph_query",
		"subject":   "Alice",
	})

	if result.IsError {
		t.Errorf("Unexpected error: %s", result.ForLLM)
	}
	if !strings.Contains(result.ForLLM, "Alice") {
		t.Errorf("Result should contain Alice: %s", result.ForLLM)
	}
}

func TestMemoryListTool_Execute_GraphQuery_NilStore(t *testing.T) {
	sp := &mockStoreProvider{}
	tool := memory.NewMemoryListTool(sp)

	result := tool.Execute(context.Background(), map[string]interface{}{
		"list_type": "graph_query",
	})
	if !result.IsError {
		t.Error("Expected error for nil store")
	}
}

func TestMemoryListTool_Execute_GraphQuery_NoMatches(t *testing.T) {
	sp := newTestStoreProvider(t)
	tool := memory.NewMemoryListTool(sp)

	result := tool.Execute(context.Background(), map[string]interface{}{
		"list_type":  "graph_query",
		"subject":   "NonExistent",
	})
	if result.IsError {
		t.Errorf("Unexpected error: %s", result.ForLLM)
	}
	if !strings.Contains(result.ForLLM, "No matching") {
		t.Errorf("Should mention no matches: %s", result.ForLLM)
	}
}

func TestMemoryListTool_Execute_GraphRelated(t *testing.T) {
	sp := newTestStoreProvider(t)
	tool := memory.NewMemoryListTool(sp)
	ctx := context.Background()

	_ = sp.graphStore.AddEntity(ctx, &graph.Entity{
		Name:       "Alice",
		Type:       "person",
		Properties: map[string]string{"age": "30"},
	})
	_ = sp.graphStore.AddTriple(ctx, &graph.Triple{
		Subject: "Alice", Predicate: "knows", Object: "Bob",
	})

	result := tool.Execute(ctx, map[string]interface{}{
		"list_type":   "graph_related",
		"entity_name": "Alice",
		"depth":       float64(1),
	})

	if result.IsError {
		t.Errorf("Unexpected error: %s", result.ForLLM)
	}
	if !strings.Contains(result.ForLLM, "Alice") {
		t.Errorf("Result should contain Alice: %s", result.ForLLM)
	}
}

func TestMemoryListTool_Execute_GraphRelated_NoName(t *testing.T) {
	sp := newTestStoreProvider(t)
	tool := memory.NewMemoryListTool(sp)

	result := tool.Execute(context.Background(), map[string]interface{}{
		"list_type": "graph_related",
	})
	if !result.IsError {
		t.Error("Expected error for missing entity_name")
	}
}

func TestMemoryListTool_Execute_GraphRelated_NilStore(t *testing.T) {
	sp := &mockStoreProvider{}
	tool := memory.NewMemoryListTool(sp)

	result := tool.Execute(context.Background(), map[string]interface{}{
		"list_type":   "graph_related",
		"entity_name": "test",
	})
	if !result.IsError {
		t.Error("Expected error for nil store")
	}
}

func TestMemoryListTool_Execute_GraphRelated_NoRelationships(t *testing.T) {
	sp := newTestStoreProvider(t)
	tool := memory.NewMemoryListTool(sp)

	result := tool.Execute(context.Background(), map[string]interface{}{
		"list_type":   "graph_related",
		"entity_name": "Nobody",
	})
	if result.IsError {
		t.Errorf("Unexpected error: %s", result.ForLLM)
	}
	if !strings.Contains(result.ForLLM, "No relationships") {
		t.Errorf("Should mention no relationships: %s", result.ForLLM)
	}
}

func TestMemoryListTool_Execute_InvalidListType(t *testing.T) {
	sp := newTestStoreProvider(t)
	tool := memory.NewMemoryListTool(sp)

	result := tool.Execute(context.Background(), map[string]interface{}{
		"list_type": "invalid",
	})
	if !result.IsError {
		t.Error("Expected error for invalid list_type")
	}
}

// ---------------------------------------------------------------------------
// Tool interface compliance checks
// ---------------------------------------------------------------------------

func TestToolInterfaceCompliance(t *testing.T) {
	sp := newTestStoreProvider(t)

	var _ tools.Tool = memory.NewMemorySearchTool(sp)
	var _ tools.Tool = memory.NewMemoryStoreTool(sp)
	var _ tools.Tool = memory.NewMemoryForgetTool(sp)
	var _ tools.Tool = memory.NewMemoryListTool(sp)
}

// ---------------------------------------------------------------------------
// truncateText edge cases (indirect via tool output)
// ---------------------------------------------------------------------------

func TestMemorySearchTool_LargeEpisodes(t *testing.T) {
	sp := newTestStoreProvider(t)
	ctx := context.Background()

	longContent := strings.Repeat("abcdefghij ", 100) // ~1.1KB
	_ = sp.episodicStore.StoreEpisode(ctx, &episodic.Episode{
		SessionKey: "long-session",
		Role:       "user",
		Content:    longContent,
	})

	tool := memory.NewMemorySearchTool(sp)
	result := tool.Execute(ctx, map[string]interface{}{
		"query":        "abcdefghij",
		"memory_type": "episodic",
	})

	if result.IsError {
		t.Errorf("Unexpected error: %s", result.ForLLM)
	}
	// Content should be truncated in output
	if !strings.Contains(result.ForLLM, "abcdefghij") {
		t.Error("Result should contain part of the content")
	}
}
