package memory_test

import (
	"context"
	"testing"
	"time"

	episodic "github.com/276793422/NemesisBot/module/memory/episodic"
)

// newTestStore creates a fresh episodic store backed by a temp directory.
func newTestStore(t *testing.T) *episodic.Store {
	t.Helper()
	return newTestStoreWithMax(t, 100)
}

func newTestStoreWithMax(t *testing.T, maxPerSession int) *episodic.Store {
	t.Helper()
	cfg := episodic.Config{
		StoragePath:           t.TempDir(),
		MaxEpisodesPerSession: maxPerSession,
		RetentionDays:         90,
	}
	s, err := episodic.NewStore(cfg)
	if err != nil {
		t.Fatalf("NewStore: %v", err)
	}
	t.Cleanup(func() { s.Close() })
	return s
}

func storeEpisode(t *testing.T, s *episodic.Store, ctx context.Context, sessionKey, role, content string) *episodic.Episode {
	t.Helper()
	ep := &episodic.Episode{
		SessionKey: sessionKey,
		Role:       role,
		Content:    content,
	}
	if err := s.StoreEpisode(ctx, ep); err != nil {
		t.Fatalf("StoreEpisode: %v", err)
	}
	return ep
}

func TestEpisodicStore_StoreAndRetrieve(t *testing.T) {
	s := newTestStore(t)
	ctx := context.Background()

	ep1 := storeEpisode(t, s, ctx, "session-1", "user", "Hello there")
	_ = storeEpisode(t, s, ctx, "session-1", "assistant", "Hi! How can I help?")

	if ep1.ID == "" {
		t.Error("expected ID to be set")
	}
	if ep1.Timestamp.IsZero() {
		t.Error("expected Timestamp to be set")
	}

	episodes, err := s.GetSession(ctx, "session-1")
	if err != nil {
		t.Fatalf("GetSession: %v", err)
	}
	if len(episodes) != 2 {
		t.Fatalf("expected 2 episodes, got %d", len(episodes))
	}
	if episodes[0].Content != "Hello there" {
		t.Errorf("first episode content: got %q", episodes[0].Content)
	}
	if episodes[1].Content != "Hi! How can I help?" {
		t.Errorf("second episode content: got %q", episodes[1].Content)
	}

	// Different session should return empty.
	other, err := s.GetSession(ctx, "session-other")
	if err != nil {
		t.Fatalf("GetSession other: %v", err)
	}
	if len(other) != 0 {
		t.Errorf("expected 0 episodes for unknown session, got %d", len(other))
	}
}

func TestEpisodicStore_GetRecent(t *testing.T) {
	s := newTestStore(t)
	ctx := context.Background()

	for i := 0; i < 10; i++ {
		storeEpisode(t, s, ctx, "session-1", "user", "msg")
	}

	// Get recent 3.
	recent, err := s.GetRecent(ctx, "session-1", 3)
	if err != nil {
		t.Fatalf("GetRecent: %v", err)
	}
	if len(recent) != 3 {
		t.Fatalf("expected 3 recent episodes, got %d", len(recent))
	}

	// Get recent with limit > total.
	recentAll, err := s.GetRecent(ctx, "session-1", 100)
	if err != nil {
		t.Fatalf("GetRecent all: %v", err)
	}
	if len(recentAll) != 10 {
		t.Errorf("expected 10 episodes, got %d", len(recentAll))
	}

	// Non-existent session.
	none, err := s.GetRecent(ctx, "no-such-session", 5)
	if err != nil {
		t.Fatalf("GetRecent missing: %v", err)
	}
	if len(none) != 0 {
		t.Errorf("expected 0 for missing session, got %d", len(none))
	}
}

func TestEpisodicStore_Search(t *testing.T) {
	s := newTestStore(t)
	ctx := context.Background()

	storeEpisode(t, s, ctx, "s1", "user", "What is the weather in Tokyo?")
	storeEpisode(t, s, ctx, "s1", "assistant", "The weather in Tokyo is sunny.")
	storeEpisode(t, s, ctx, "s2", "user", "How is the economy doing?")
	storeEpisode(t, s, ctx, "s2", "assistant", "The economy is stable.")

	// Search for "tokyo" (case-insensitive).
	results, err := s.Search(ctx, "tokyo", 10)
	if err != nil {
		t.Fatalf("Search: %v", err)
	}
	if len(results) != 2 {
		t.Errorf("expected 2 results for 'tokyo', got %d", len(results))
	}

	// Search for "economy".
	results2, err := s.Search(ctx, "economy", 10)
	if err != nil {
		t.Fatalf("Search: %v", err)
	}
	if len(results2) != 2 {
		t.Errorf("expected 2 results for 'economy', got %d", len(results2))
	}

	// Search with limit.
	results3, err := s.Search(ctx, "tokyo", 1)
	if err != nil {
		t.Fatalf("Search: %v", err)
	}
	if len(results3) != 1 {
		t.Errorf("expected 1 result with limit=1, got %d", len(results3))
	}

	// Search by tag.
	s2 := storeEpisode(t, s, ctx, "s3", "user", "some content")
	s2.Tags = []string{"important-tag"}
	// Manually re-store with tags (storeEpisode sets tags before store).
	ep := &episodic.Episode{
		SessionKey: "s3",
		Role:       "user",
		Content:    "tagged content here",
		Tags:       []string{"important-tag"},
	}
	if err := s.StoreEpisode(ctx, ep); err != nil {
		t.Fatalf("StoreEpisode tagged: %v", err)
	}

	results4, err := s.Search(ctx, "important-tag", 10)
	if err != nil {
		t.Fatalf("Search tag: %v", err)
	}
	if len(results4) < 1 {
		t.Error("expected at least 1 result searching by tag")
	}
}

func TestEpisodicStore_DeleteSession(t *testing.T) {
	s := newTestStore(t)
	ctx := context.Background()

	storeEpisode(t, s, ctx, "to-delete", "user", "delete me")
	storeEpisode(t, s, ctx, "to-delete", "assistant", "ok, deleted")

	if s.SessionCount() != 1 {
		t.Fatalf("expected 1 session, got %d", s.SessionCount())
	}

	if err := s.DeleteSession(ctx, "to-delete"); err != nil {
		t.Fatalf("DeleteSession: %v", err)
	}

	if s.SessionCount() != 0 {
		t.Errorf("expected 0 sessions after delete, got %d", s.SessionCount())
	}

	episodes, err := s.GetSession(ctx, "to-delete")
	if err != nil {
		t.Fatalf("GetSession after delete: %v", err)
	}
	if len(episodes) != 0 {
		t.Errorf("expected 0 episodes after delete, got %d", len(episodes))
	}
}

func TestEpisodicStore_Cleanup(t *testing.T) {
	s := newTestStore(t)
	ctx := context.Background()

	// Store old episode by manually setting timestamp.
	oldEp := &episodic.Episode{
		SessionKey: "old-session",
		Role:       "user",
		Content:    "old content",
		Timestamp:  time.Now().UTC().Add(-200 * 24 * time.Hour), // 200 days ago
	}
	if err := s.StoreEpisode(ctx, oldEp); err != nil {
		t.Fatalf("StoreEpisode old: %v", err)
	}

	// Store recent episode.
	storeEpisode(t, s, ctx, "new-session", "user", "recent content")

	if s.SessionCount() != 2 {
		t.Fatalf("expected 2 sessions, got %d", s.SessionCount())
	}

	// Cleanup entries older than 90 days.
	removed, err := s.Cleanup(ctx, 90*24*time.Hour)
	if err != nil {
		t.Fatalf("Cleanup: %v", err)
	}
	if removed != 1 {
		t.Errorf("expected 1 removed, got %d", removed)
	}

	// Only the recent session should remain.
	if s.SessionCount() != 1 {
		t.Errorf("expected 1 session after cleanup, got %d", s.SessionCount())
	}
	if s.EpisodeCount() != 1 {
		t.Errorf("expected 1 episode after cleanup, got %d", s.EpisodeCount())
	}
}

func TestEpisodicStore_MaxPerSession(t *testing.T) {
	max := 5
	s := newTestStoreWithMax(t, max)
	ctx := context.Background()

	// Store 10 episodes.
	for i := 0; i < 10; i++ {
		storeEpisode(t, s, ctx, "limited-session", "user", "msg")
	}

	episodes, err := s.GetSession(ctx, "limited-session")
	if err != nil {
		t.Fatalf("GetSession: %v", err)
	}
	if len(episodes) != max {
		t.Errorf("expected at most %d episodes, got %d", max, len(episodes))
	}
}

func TestEpisodicStore_StoreNilEpisode(t *testing.T) {
	s := newTestStore(t)
	ctx := context.Background()

	err := s.StoreEpisode(ctx, nil)
	if err == nil {
		t.Error("expected error for nil episode")
	}
}

func TestEpisodicStore_StoreEmptySessionKey(t *testing.T) {
	s := newTestStore(t)
	ctx := context.Background()

	err := s.StoreEpisode(ctx, &episodic.Episode{Content: "no session"})
	if err == nil {
		t.Error("expected error for empty session key")
	}
}

func TestEpisodicStore_SessionAndEpisodeCount(t *testing.T) {
	s := newTestStore(t)
	ctx := context.Background()

	if s.SessionCount() != 0 {
		t.Errorf("expected 0 sessions, got %d", s.SessionCount())
	}
	if s.EpisodeCount() != 0 {
		t.Errorf("expected 0 episodes, got %d", s.EpisodeCount())
	}

	storeEpisode(t, s, ctx, "s1", "user", "a")
	storeEpisode(t, s, ctx, "s1", "user", "b")
	storeEpisode(t, s, ctx, "s2", "user", "c")

	if s.SessionCount() != 2 {
		t.Errorf("expected 2 sessions, got %d", s.SessionCount())
	}
	if s.EpisodeCount() != 3 {
		t.Errorf("expected 3 episodes, got %d", s.EpisodeCount())
	}
}

func TestEpisodicStore_Persistence(t *testing.T) {
	tmp := t.TempDir()

	// Store data.
	cfg := episodic.Config{StoragePath: tmp, MaxEpisodesPerSession: 100, RetentionDays: 90}
	s1, err := episodic.NewStore(cfg)
	if err != nil {
		t.Fatalf("NewStore: %v", err)
	}
	ctx := context.Background()
	storeEpisode(t, s1, ctx, "persist", "user", "persistent message")
	s1.Close()

	// Reload.
	s2, err := episodic.NewStore(cfg)
	if err != nil {
		t.Fatalf("NewStore reload: %v", err)
	}
	defer s2.Close()

	episodes, err := s2.GetSession(ctx, "persist")
	if err != nil {
		t.Fatalf("GetSession: %v", err)
	}
	if len(episodes) != 1 {
		t.Fatalf("expected 1 episode after reload, got %d", len(episodes))
	}
	if episodes[0].Content != "persistent message" {
		t.Errorf("content mismatch: got %q", episodes[0].Content)
	}
}
