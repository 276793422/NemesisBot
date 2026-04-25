package episodic_test

import (
	"context"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/276793422/NemesisBot/module/memory/episodic"
)

// ---------------------------------------------------------------------------
// NewStore
// ---------------------------------------------------------------------------

func TestNewStore_CreatesDirectory(t *testing.T) {
	dir := t.TempDir()
	storagePath := dir + "/episodic"

	store, err := episodic.NewStore(episodic.Config{
		StoragePath:           storagePath,
		MaxEpisodesPerSession: 100,
		RetentionDays:         90,
	})
	if err != nil {
		t.Fatalf("NewStore: %v", err)
	}
	defer store.Close()

	if store.SessionCount() != 0 {
		t.Error("New store should have 0 sessions")
	}
}

func TestNewStore_DefaultConfig(t *testing.T) {
	dir := t.TempDir()

	// Zero values should be replaced by defaults
	store, err := episodic.NewStore(episodic.Config{
		StoragePath:           dir,
		MaxEpisodesPerSession: 0, // should default to 100
		RetentionDays:         0, // should default to 90
	})
	if err != nil {
		t.Fatalf("NewStore with zero config: %v", err)
	}
	defer store.Close()
}

func TestNewStore_LoadsExisting(t *testing.T) {
	dir := t.TempDir()
	ctx := context.Background()

	// Create store and add data
	store1, err := episodic.NewStore(episodic.Config{
		StoragePath:           dir,
		MaxEpisodesPerSession: 100,
		RetentionDays:         90,
	})
	if err != nil {
		t.Fatalf("NewStore 1: %v", err)
	}
	_ = store1.StoreEpisode(ctx, &episodic.Episode{
		SessionKey: "persist-test",
		Role:       "user",
		Content:    "persistent content",
	})
	store1.Close()

	// Create new store from same directory
	store2, err := episodic.NewStore(episodic.Config{
		StoragePath:           dir,
		MaxEpisodesPerSession: 100,
		RetentionDays:         90,
	})
	if err != nil {
		t.Fatalf("NewStore 2: %v", err)
	}
	defer store2.Close()

	episodes, err := store2.GetSession(ctx, "persist-test")
	if err != nil {
		t.Fatalf("GetSession: %v", err)
	}
	if len(episodes) != 1 {
		t.Fatalf("Expected 1 episode, got %d", len(episodes))
	}
	if episodes[0].Content != "persistent content" {
		t.Errorf("Content = %q, want %q", episodes[0].Content, "persistent content")
	}
}

// ---------------------------------------------------------------------------
// StoreEpisode
// ---------------------------------------------------------------------------

func TestStore_StoreEpisode(t *testing.T) {
	dir := t.TempDir()
	store, err := episodic.NewStore(episodic.Config{
		StoragePath:           dir,
		MaxEpisodesPerSession: 100,
		RetentionDays:         90,
	})
	if err != nil {
		t.Fatalf("NewStore: %v", err)
	}
	defer store.Close()

	ctx := context.Background()
	ep := &episodic.Episode{
		SessionKey: "test-session",
		Role:       "user",
		Content:    "Hello world",
		Tags:       []string{"greeting"},
		Metadata:   map[string]string{"source": "test"},
	}

	if err := store.StoreEpisode(ctx, ep); err != nil {
		t.Fatalf("StoreEpisode: %v", err)
	}

	// ID should be auto-generated
	if ep.ID == "" {
		t.Error("Episode ID should be auto-generated")
	}
	// Timestamp should be auto-set
	if ep.Timestamp.IsZero() {
		t.Error("Episode Timestamp should be auto-set")
	}

	if store.EpisodeCount() != 1 {
		t.Errorf("EpisodeCount = %d, want 1", store.EpisodeCount())
	}
	if store.SessionCount() != 1 {
		t.Errorf("SessionCount = %d, want 1", store.SessionCount())
	}
}

func TestStore_StoreEpisode_Nil(t *testing.T) {
	dir := t.TempDir()
	store, _ := episodic.NewStore(episodic.Config{
		StoragePath:           dir,
		MaxEpisodesPerSession: 100,
		RetentionDays:         90,
	})
	defer store.Close()

	err := store.StoreEpisode(context.Background(), nil)
	if err == nil {
		t.Error("Expected error for nil episode")
	}
}

func TestStore_StoreEpisode_EmptySession(t *testing.T) {
	dir := t.TempDir()
	store, _ := episodic.NewStore(episodic.Config{
		StoragePath:           dir,
		MaxEpisodesPerSession: 100,
		RetentionDays:         90,
	})
	defer store.Close()

	err := store.StoreEpisode(context.Background(), &episodic.Episode{
		Content: "no session",
	})
	if err == nil {
		t.Error("Expected error for empty session key")
	}
}

func TestStore_StoreEpisode_PreservesID(t *testing.T) {
	dir := t.TempDir()
	store, _ := episodic.NewStore(episodic.Config{
		StoragePath:           dir,
		MaxEpisodesPerSession: 100,
		RetentionDays:         90,
	})
	defer store.Close()

	ep := &episodic.Episode{
		ID:         "custom-id-123",
		SessionKey: "session",
		Content:    "test",
		Role:       "user",
	}
	_ = store.StoreEpisode(context.Background(), ep)

	if ep.ID != "custom-id-123" {
		t.Errorf("ID should be preserved, got %q", ep.ID)
	}
}

func TestStore_StoreEpisode_PreservesTimestamp(t *testing.T) {
	dir := t.TempDir()
	store, _ := episodic.NewStore(episodic.Config{
		StoragePath:           dir,
		MaxEpisodesPerSession: 100,
		RetentionDays:         90,
	})
	defer store.Close()

	ts := time.Date(2025, 6, 15, 10, 30, 0, 0, time.UTC)
	ep := &episodic.Episode{
		SessionKey: "session",
		Content:    "test",
		Role:       "user",
		Timestamp:  ts,
	}
	_ = store.StoreEpisode(context.Background(), ep)

	if !ep.Timestamp.Equal(ts) {
		t.Errorf("Timestamp = %v, want %v", ep.Timestamp, ts)
	}
}

func TestStore_StoreEpisode_MaxEpisodesPerSession(t *testing.T) {
	dir := t.TempDir()
	store, _ := episodic.NewStore(episodic.Config{
		StoragePath:           dir,
		MaxEpisodesPerSession: 3,
		RetentionDays:         90,
	})
	defer store.Close()

	ctx := context.Background()

	// Store 5 episodes in same session
	for i := 0; i < 5; i++ {
		_ = store.StoreEpisode(ctx, &episodic.Episode{
			SessionKey: "limited-session",
			Content:    "content",
			Role:       "user",
		})
	}

	episodes, _ := store.GetSession(ctx, "limited-session")
	if len(episodes) != 3 {
		t.Errorf("Expected 3 episodes (max), got %d", len(episodes))
	}
}

// ---------------------------------------------------------------------------
// GetSession
// ---------------------------------------------------------------------------

func TestStore_GetSession_Existing(t *testing.T) {
	dir := t.TempDir()
	store, _ := episodic.NewStore(episodic.Config{
		StoragePath:           dir,
		MaxEpisodesPerSession: 100,
		RetentionDays:         90,
	})
	defer store.Close()

	ctx := context.Background()
	_ = store.StoreEpisode(ctx, &episodic.Episode{SessionKey: "s1", Role: "user", Content: "msg1"})
	_ = store.StoreEpisode(ctx, &episodic.Episode{SessionKey: "s1", Role: "assistant", Content: "msg2"})

	episodes, err := store.GetSession(ctx, "s1")
	if err != nil {
		t.Fatalf("GetSession: %v", err)
	}
	if len(episodes) != 2 {
		t.Errorf("Expected 2 episodes, got %d", len(episodes))
	}
}

func TestStore_GetSession_NonExisting(t *testing.T) {
	dir := t.TempDir()
	store, _ := episodic.NewStore(episodic.Config{
		StoragePath:           dir,
		MaxEpisodesPerSession: 100,
		RetentionDays:         90,
	})
	defer store.Close()

	episodes, err := store.GetSession(context.Background(), "nonexistent")
	if err != nil {
		t.Fatalf("GetSession: %v", err)
	}
	if len(episodes) != 0 {
		t.Errorf("Expected 0 episodes, got %d", len(episodes))
	}
}

// ---------------------------------------------------------------------------
// GetRecent
// ---------------------------------------------------------------------------

func TestStore_GetRecent(t *testing.T) {
	dir := t.TempDir()
	store, _ := episodic.NewStore(episodic.Config{
		StoragePath:           dir,
		MaxEpisodesPerSession: 100,
		RetentionDays:         90,
	})
	defer store.Close()

	ctx := context.Background()
	for i := 0; i < 10; i++ {
		_ = store.StoreEpisode(ctx, &episodic.Episode{
			SessionKey: "recent-session",
			Role:       "user",
			Content:    "msg",
		})
	}

	// Get last 3
	episodes, err := store.GetRecent(ctx, "recent-session", 3)
	if err != nil {
		t.Fatalf("GetRecent: %v", err)
	}
	if len(episodes) != 3 {
		t.Errorf("Expected 3 episodes, got %d", len(episodes))
	}
}

func TestStore_GetRecent_LimitLargerThanTotal(t *testing.T) {
	dir := t.TempDir()
	store, _ := episodic.NewStore(episodic.Config{
		StoragePath:           dir,
		MaxEpisodesPerSession: 100,
		RetentionDays:         90,
	})
	defer store.Close()

	ctx := context.Background()
	_ = store.StoreEpisode(ctx, &episodic.Episode{
		SessionKey: "small-session",
		Role:       "user",
		Content:    "only one",
	})

	episodes, err := store.GetRecent(ctx, "small-session", 10)
	if err != nil {
		t.Fatalf("GetRecent: %v", err)
	}
	if len(episodes) != 1 {
		t.Errorf("Expected 1 episode, got %d", len(episodes))
	}
}

func TestStore_GetRecent_NonExistingSession(t *testing.T) {
	dir := t.TempDir()
	store, _ := episodic.NewStore(episodic.Config{
		StoragePath:           dir,
		MaxEpisodesPerSession: 100,
		RetentionDays:         90,
	})
	defer store.Close()

	episodes, err := store.GetRecent(context.Background(), "nonexistent", 5)
	if err != nil {
		t.Fatalf("GetRecent: %v", err)
	}
	if len(episodes) != 0 {
		t.Errorf("Expected 0 episodes, got %d", len(episodes))
	}
}

func TestStore_GetRecent_ZeroLimit(t *testing.T) {
	dir := t.TempDir()
	store, _ := episodic.NewStore(episodic.Config{
		StoragePath:           dir,
		MaxEpisodesPerSession: 100,
		RetentionDays:         90,
	})
	defer store.Close()

	ctx := context.Background()
	for i := 0; i < 15; i++ {
		_ = store.StoreEpisode(ctx, &episodic.Episode{
			SessionKey: "zero-limit",
			Role:       "user",
			Content:    "msg",
		})
	}

	// Zero limit should default to 10
	episodes, err := store.GetRecent(ctx, "zero-limit", 0)
	if err != nil {
		t.Fatalf("GetRecent: %v", err)
	}
	if len(episodes) != 10 {
		t.Errorf("Zero limit should default to 10, got %d", len(episodes))
	}
}

// ---------------------------------------------------------------------------
// Search
// ---------------------------------------------------------------------------

func TestStore_Search_ByContent(t *testing.T) {
	dir := t.TempDir()
	store, _ := episodic.NewStore(episodic.Config{
		StoragePath:           dir,
		MaxEpisodesPerSession: 100,
		RetentionDays:         90,
	})
	defer store.Close()

	ctx := context.Background()
	_ = store.StoreEpisode(ctx, &episodic.Episode{SessionKey: "s1", Role: "user", Content: "Python is great"})
	_ = store.StoreEpisode(ctx, &episodic.Episode{SessionKey: "s2", Role: "user", Content: "Go is fast"})
	_ = store.StoreEpisode(ctx, &episodic.Episode{SessionKey: "s3", Role: "user", Content: "Rust is safe"})

	results, err := store.Search(ctx, "python", 10)
	if err != nil {
		t.Fatalf("Search: %v", err)
	}
	if len(results) != 1 {
		t.Errorf("Expected 1 result, got %d", len(results))
	}
	if len(results) > 0 && !strings.Contains(results[0].Content, "Python") {
		t.Errorf("Result content = %q, should contain Python", results[0].Content)
	}
}

func TestStore_Search_ByTag(t *testing.T) {
	dir := t.TempDir()
	store, _ := episodic.NewStore(episodic.Config{
		StoragePath:           dir,
		MaxEpisodesPerSession: 100,
		RetentionDays:         90,
	})
	defer store.Close()

	ctx := context.Background()
	_ = store.StoreEpisode(ctx, &episodic.Episode{
		SessionKey: "s1",
		Role:       "user",
		Content:    "generic content",
		Tags:       []string{"important"},
	})

	results, err := store.Search(ctx, "important", 10)
	if err != nil {
		t.Fatalf("Search: %v", err)
	}
	if len(results) != 1 {
		t.Errorf("Expected 1 result for tag match, got %d", len(results))
	}
}

func TestStore_Search_CaseInsensitive(t *testing.T) {
	dir := t.TempDir()
	store, _ := episodic.NewStore(episodic.Config{
		StoragePath:           dir,
		MaxEpisodesPerSession: 100,
		RetentionDays:         90,
	})
	defer store.Close()

	ctx := context.Background()
	_ = store.StoreEpisode(ctx, &episodic.Episode{
		SessionKey: "s1",
		Role:       "user",
		Content:    "UPPERCASE CONTENT",
	})

	results, err := store.Search(ctx, "uppercase", 10)
	if err != nil {
		t.Fatalf("Search: %v", err)
	}
	if len(results) != 1 {
		t.Errorf("Expected 1 result (case insensitive), got %d", len(results))
	}
}

func TestStore_Search_Limit(t *testing.T) {
	dir := t.TempDir()
	store, _ := episodic.NewStore(episodic.Config{
		StoragePath:           dir,
		MaxEpisodesPerSession: 100,
		RetentionDays:         90,
	})
	defer store.Close()

	ctx := context.Background()
	for i := 0; i < 10; i++ {
		_ = store.StoreEpisode(ctx, &episodic.Episode{
			SessionKey: "s1",
			Role:       "user",
			Content:    "common keyword here",
		})
	}

	results, err := store.Search(ctx, "keyword", 3)
	if err != nil {
		t.Fatalf("Search: %v", err)
	}
	if len(results) > 3 {
		t.Errorf("Expected at most 3 results, got %d", len(results))
	}
}

func TestStore_Search_NoResults(t *testing.T) {
	dir := t.TempDir()
	store, _ := episodic.NewStore(episodic.Config{
		StoragePath:           dir,
		MaxEpisodesPerSession: 100,
		RetentionDays:         90,
	})
	defer store.Close()

	results, err := store.Search(context.Background(), "nonexistent_xyz", 10)
	if err != nil {
		t.Fatalf("Search: %v", err)
	}
	if len(results) != 0 {
		t.Errorf("Expected 0 results, got %d", len(results))
	}
}

func TestStore_Search_ZeroLimit(t *testing.T) {
	dir := t.TempDir()
	store, _ := episodic.NewStore(episodic.Config{
		StoragePath:           dir,
		MaxEpisodesPerSession: 100,
		RetentionDays:         90,
	})
	defer store.Close()

	ctx := context.Background()
	_ = store.StoreEpisode(ctx, &episodic.Episode{
		SessionKey: "s1",
		Role:       "user",
		Content:    "test content",
	})

	// Zero limit should default to 20
	results, err := store.Search(ctx, "test", 0)
	if err != nil {
		t.Fatalf("Search: %v", err)
	}
	if len(results) != 1 {
		t.Errorf("Expected 1 result, got %d", len(results))
	}
}

// ---------------------------------------------------------------------------
// DeleteSession
// ---------------------------------------------------------------------------

func TestStore_DeleteSession(t *testing.T) {
	dir := t.TempDir()
	store, _ := episodic.NewStore(episodic.Config{
		StoragePath:           dir,
		MaxEpisodesPerSession: 100,
		RetentionDays:         90,
	})
	defer store.Close()

	ctx := context.Background()
	_ = store.StoreEpisode(ctx, &episodic.Episode{
		SessionKey: "delete-me",
		Role:       "user",
		Content:    "to be deleted",
	})

	if err := store.DeleteSession(ctx, "delete-me"); err != nil {
		t.Fatalf("DeleteSession: %v", err)
	}

	episodes, _ := store.GetSession(ctx, "delete-me")
	if len(episodes) != 0 {
		t.Error("Session should be empty after deletion")
	}
}

func TestStore_DeleteSession_NonExisting(t *testing.T) {
	dir := t.TempDir()
	store, _ := episodic.NewStore(episodic.Config{
		StoragePath:           dir,
		MaxEpisodesPerSession: 100,
		RetentionDays:         90,
	})
	defer store.Close()

	// Should not error for non-existing session
	err := store.DeleteSession(context.Background(), "nonexistent")
	if err != nil {
		t.Errorf("DeleteSession non-existing should not error: %v", err)
	}
}

// ---------------------------------------------------------------------------
// Cleanup
// ---------------------------------------------------------------------------

func TestStore_Cleanup(t *testing.T) {
	dir := t.TempDir()
	store, _ := episodic.NewStore(episodic.Config{
		StoragePath:           dir,
		MaxEpisodesPerSession: 100,
		RetentionDays:         90,
	})
	defer store.Close()

	ctx := context.Background()

	// Store an old episode
	_ = store.StoreEpisode(ctx, &episodic.Episode{
		SessionKey: "old-session",
		Role:       "user",
		Content:    "old content",
		Timestamp:  time.Now().UTC().Add(-200 * 24 * time.Hour),
	})

	// Store a recent episode
	_ = store.StoreEpisode(ctx, &episodic.Episode{
		SessionKey: "recent-session",
		Role:       "user",
		Content:    "recent content",
		Timestamp:  time.Now().UTC(),
	})

	removed, err := store.Cleanup(ctx, 90*24*time.Hour)
	if err != nil {
		t.Fatalf("Cleanup: %v", err)
	}
	if removed != 1 {
		t.Errorf("Expected 1 removed, got %d", removed)
	}

	// Old session should be gone
	episodes, _ := store.GetSession(ctx, "old-session")
	if len(episodes) != 0 {
		t.Error("Old session should be cleaned up")
	}

	// Recent session should remain
	episodes2, _ := store.GetSession(ctx, "recent-session")
	if len(episodes2) != 1 {
		t.Error("Recent session should remain")
	}
}

func TestStore_Cleanup_NothingToRemove(t *testing.T) {
	dir := t.TempDir()
	store, _ := episodic.NewStore(episodic.Config{
		StoragePath:           dir,
		MaxEpisodesPerSession: 100,
		RetentionDays:         90,
	})
	defer store.Close()

	ctx := context.Background()
	_ = store.StoreEpisode(ctx, &episodic.Episode{
		SessionKey: "fresh-session",
		Role:       "user",
		Content:    "fresh",
	})

	removed, err := store.Cleanup(ctx, 365*24*time.Hour) // 1 year
	if err != nil {
		t.Fatalf("Cleanup: %v", err)
	}
	if removed != 0 {
		t.Errorf("Expected 0 removed, got %d", removed)
	}
}

func TestStore_Cleanup_PartialSession(t *testing.T) {
	dir := t.TempDir()
	store, _ := episodic.NewStore(episodic.Config{
		StoragePath:           dir,
		MaxEpisodesPerSession: 100,
		RetentionDays:         90,
	})
	defer store.Close()

	ctx := context.Background()

	// Mix of old and recent in same session
	_ = store.StoreEpisode(ctx, &episodic.Episode{
		SessionKey: "mixed-session",
		Role:       "user",
		Content:    "old entry",
		Timestamp:  time.Now().UTC().Add(-200 * 24 * time.Hour),
	})
	_ = store.StoreEpisode(ctx, &episodic.Episode{
		SessionKey: "mixed-session",
		Role:       "user",
		Content:    "recent entry",
		Timestamp:  time.Now().UTC(),
	})

	removed, err := store.Cleanup(ctx, 90*24*time.Hour)
	if err != nil {
		t.Fatalf("Cleanup: %v", err)
	}
	if removed != 1 {
		t.Errorf("Expected 1 removed, got %d", removed)
	}

	// Session should still exist with 1 episode
	episodes, _ := store.GetSession(ctx, "mixed-session")
	if len(episodes) != 1 {
		t.Errorf("Expected 1 remaining, got %d", len(episodes))
	}
}

// ---------------------------------------------------------------------------
// SessionCount and EpisodeCount
// ---------------------------------------------------------------------------

func TestStore_Counts(t *testing.T) {
	dir := t.TempDir()
	store, _ := episodic.NewStore(episodic.Config{
		StoragePath:           dir,
		MaxEpisodesPerSession: 100,
		RetentionDays:         90,
	})
	defer store.Close()

	ctx := context.Background()

	if store.SessionCount() != 0 {
		t.Error("New store should have 0 sessions")
	}
	if store.EpisodeCount() != 0 {
		t.Error("New store should have 0 episodes")
	}

	_ = store.StoreEpisode(ctx, &episodic.Episode{SessionKey: "s1", Role: "user", Content: "a"})
	_ = store.StoreEpisode(ctx, &episodic.Episode{SessionKey: "s1", Role: "assistant", Content: "b"})
	_ = store.StoreEpisode(ctx, &episodic.Episode{SessionKey: "s2", Role: "user", Content: "c"})

	if store.SessionCount() != 2 {
		t.Errorf("SessionCount = %d, want 2", store.SessionCount())
	}
	if store.EpisodeCount() != 3 {
		t.Errorf("EpisodeCount = %d, want 3", store.EpisodeCount())
	}
}

// ---------------------------------------------------------------------------
// Close
// ---------------------------------------------------------------------------

func TestStore_Close(t *testing.T) {
	dir := t.TempDir()
	store, _ := episodic.NewStore(episodic.Config{
		StoragePath:           dir,
		MaxEpisodesPerSession: 100,
		RetentionDays:         90,
	})

	if err := store.Close(); err != nil {
		t.Fatalf("Close: %v", err)
	}
}

// ---------------------------------------------------------------------------
// Concurrent access
// ---------------------------------------------------------------------------

func TestStore_ConcurrentAccess(t *testing.T) {
	dir := t.TempDir()
	store, _ := episodic.NewStore(episodic.Config{
		StoragePath:           dir,
		MaxEpisodesPerSession: 1000,
		RetentionDays:         90,
	})
	defer store.Close()

	ctx := context.Background()
	var wg sync.WaitGroup
	numGoroutines := 20

	for i := 0; i < numGoroutines; i++ {
		wg.Add(1)
		go func(idx int) {
			defer wg.Done()
			_ = store.StoreEpisode(ctx, &episodic.Episode{
				SessionKey: "concurrent",
				Role:       "user",
				Content:    "concurrent content",
			})
		}(i)
	}

	wg.Wait()

	episodes, _ := store.GetSession(ctx, "concurrent")
	if len(episodes) != numGoroutines {
		t.Errorf("Expected %d episodes, got %d", numGoroutines, len(episodes))
	}
}

// ---------------------------------------------------------------------------
// Edge cases
// ---------------------------------------------------------------------------

func TestStore_SpecialCharacters(t *testing.T) {
	dir := t.TempDir()
	store, _ := episodic.NewStore(episodic.Config{
		StoragePath:           dir,
		MaxEpisodesPerSession: 100,
		RetentionDays:         90,
	})
	defer store.Close()

	ctx := context.Background()
	special := "Special: <>&\"'\\n\\t 中文 🎉 \x00"
	ep := &episodic.Episode{
		SessionKey: "special",
		Role:       "user",
		Content:    special,
	}
	if err := store.StoreEpisode(ctx, ep); err != nil {
		t.Fatalf("StoreEpisode special chars: %v", err)
	}

	episodes, _ := store.GetSession(ctx, "special")
	if len(episodes) != 1 {
		t.Fatalf("Expected 1 episode, got %d", len(episodes))
	}
	if episodes[0].Content != special {
		t.Errorf("Content mismatch: got %q, want %q", episodes[0].Content, special)
	}
}

func TestStore_VeryLongContent(t *testing.T) {
	dir := t.TempDir()
	store, _ := episodic.NewStore(episodic.Config{
		StoragePath:           dir,
		MaxEpisodesPerSession: 100,
		RetentionDays:         90,
	})
	defer store.Close()

	ctx := context.Background()
	longContent := strings.Repeat("a", 100000)
	ep := &episodic.Episode{
		SessionKey: "long",
		Role:       "user",
		Content:    longContent,
	}
	if err := store.StoreEpisode(ctx, ep); err != nil {
		t.Fatalf("StoreEpisode long content: %v", err)
	}

	episodes, _ := store.GetSession(ctx, "long")
	if len(episodes) != 1 {
		t.Fatalf("Expected 1 episode, got %d", len(episodes))
	}
	if len(episodes[0].Content) != 100000 {
		t.Errorf("Content length = %d, want 100000", len(episodes[0].Content))
	}
}

func TestStore_EmptyContent(t *testing.T) {
	dir := t.TempDir()
	store, _ := episodic.NewStore(episodic.Config{
		StoragePath:           dir,
		MaxEpisodesPerSession: 100,
		RetentionDays:         90,
	})
	defer store.Close()

	ctx := context.Background()
	ep := &episodic.Episode{
		SessionKey: "empty-content",
		Role:       "user",
		Content:    "",
	}
	if err := store.StoreEpisode(ctx, ep); err != nil {
		t.Fatalf("StoreEpisode empty content: %v", err)
	}
	// Empty content is allowed
	episodes, _ := store.GetSession(ctx, "empty-content")
	if len(episodes) != 1 {
		t.Errorf("Expected 1 episode, got %d", len(episodes))
	}
}
