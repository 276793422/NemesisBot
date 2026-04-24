// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
package episodic

import (
	"bufio"
	"context"
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"time"
)

// Episode represents a single conversation episode/experience.
type Episode struct {
	ID        string            `json:"id"`
	SessionKey string           `json:"session_key"`
	Role      string            `json:"role"` // user, assistant, system
	Content   string            `json:"content"`
	Timestamp time.Time         `json:"timestamp"`
	Metadata  map[string]string `json:"metadata,omitempty"`
	Tags      []string          `json:"tags,omitempty"`
}

// Config holds episodic store configuration.
type Config struct {
	StoragePath           string // root directory for JSONL files
	MaxEpisodesPerSession int    // default 100
	RetentionDays         int    // default 90
}

// Store implements episodic memory backed by per-session JSONL files.
type Store struct {
	path     string
	cfg      Config
	mu       sync.RWMutex
	episodes map[string][]*Episode // sessionKey -> episodes
}

// NewStore creates a new episodic store, creates directories, and loads existing data.
func NewStore(cfg Config) (*Store, error) {
	if cfg.MaxEpisodesPerSession <= 0 {
		cfg.MaxEpisodesPerSession = 100
	}
	if cfg.RetentionDays <= 0 {
		cfg.RetentionDays = 90
	}

	sessionsDir := filepath.Join(cfg.StoragePath, "sessions")
	if err := os.MkdirAll(sessionsDir, 0755); err != nil {
		return nil, fmt.Errorf("episodic: create sessions dir: %w", err)
	}

	s := &Store{
		path:     sessionsDir,
		cfg:      cfg,
		episodes: make(map[string][]*Episode),
	}

	if err := s.loadAll(); err != nil {
		return nil, fmt.Errorf("episodic: load existing: %w", err)
	}

	return s, nil
}

// loadAll reads all session JSONL files from disk into memory.
func (s *Store) loadAll() error {
	entries, err := os.ReadDir(s.path)
	if err != nil {
		if os.IsNotExist(err) {
			return nil
		}
		return err
	}

	for _, entry := range entries {
		if entry.IsDir() || !strings.HasSuffix(entry.Name(), ".jsonl") {
			continue
		}

		sessionKey := strings.TrimSuffix(entry.Name(), ".jsonl")
		episodes, err := s.loadSessionFile(filepath.Join(s.path, entry.Name()))
		if err != nil {
			continue // skip corrupted files
		}
		if len(episodes) > 0 {
			s.episodes[sessionKey] = episodes
		}
	}

	return nil
}

// loadSessionFile reads a single session JSONL file.
func (s *Store) loadSessionFile(path string) ([]*Episode, error) {
	f, err := os.Open(path)
	if err != nil {
		return nil, err
	}
	defer f.Close()

	var episodes []*Episode
	decoder := json.NewDecoder(f)
	for decoder.More() {
		var ep Episode
		if err := decoder.Decode(&ep); err != nil {
			break // stop at first decode error
		}
		episodes = append(episodes, &ep)
	}

	return episodes, nil
}

// StoreEpisode saves an episode and appends it to the session JSONL file.
func (s *Store) StoreEpisode(ctx context.Context, episode *Episode) error {
	if episode == nil {
		return fmt.Errorf("episodic: episode is nil")
	}
	if episode.SessionKey == "" {
		return fmt.Errorf("episodic: session key is required")
	}
	if episode.ID == "" {
		episode.ID = fmt.Sprintf("ep-%d", time.Now().UnixNano())
	}
	if episode.Timestamp.IsZero() {
		episode.Timestamp = time.Now().UTC()
	}

	s.mu.Lock()
	defer s.mu.Unlock()

	// Append to in-memory store
	s.episodes[episode.SessionKey] = append(s.episodes[episode.SessionKey], episode)

	// Enforce max episodes per session
	if len(s.episodes[episode.SessionKey]) > s.cfg.MaxEpisodesPerSession {
		s.episodes[episode.SessionKey] = s.episodes[episode.SessionKey][len(s.episodes[episode.SessionKey])-s.cfg.MaxEpisodesPerSession:]
	}

	// Persist to disk (append to JSONL)
	return s.appendToJSONL(episode.SessionKey, episode)
}

// appendToJSONL appends a single episode to its session JSONL file.
func (s *Store) appendToJSONL(sessionKey string, episode *Episode) error {
	path := filepath.Join(s.path, sessionKey+".jsonl")
	f, err := os.OpenFile(path, os.O_CREATE|os.O_WRONLY|os.O_APPEND, 0644)
	if err != nil {
		return fmt.Errorf("episodic: open session file: %w", err)
	}
	defer f.Close()

	data, err := json.Marshal(episode)
	if err != nil {
		return fmt.Errorf("episodic: marshal episode: %w", err)
	}

	if _, err := f.Write(append(data, '\n')); err != nil {
		return fmt.Errorf("episodic: write episode: %w", err)
	}

	return nil
}

// rewriteSessionJSONL rewrites the entire session JSONL file (used after cleanup/trim).
func (s *Store) rewriteSessionJSONL(sessionKey string) error {
	episodes, ok := s.episodes[sessionKey]
	if !ok {
		return nil
	}

	path := filepath.Join(s.path, sessionKey+".jsonl")
	f, err := os.Create(path)
	if err != nil {
		return fmt.Errorf("episodic: create session file: %w", err)
	}
	defer f.Close()

	writer := bufio.NewWriter(f)
	for _, ep := range episodes {
		data, err := json.Marshal(ep)
		if err != nil {
			continue
		}
		if _, err := writer.Write(append(data, '\n')); err != nil {
			return fmt.Errorf("episodic: write session file: %w", err)
		}
	}

	return writer.Flush()
}

// GetSession returns all episodes for a given session key.
func (s *Store) GetSession(ctx context.Context, sessionKey string) ([]*Episode, error) {
	s.mu.RLock()
	defer s.mu.RUnlock()

	episodes, ok := s.episodes[sessionKey]
	if !ok {
		return []*Episode{}, nil
	}

	// Return a copy to prevent external mutation
	result := make([]*Episode, len(episodes))
	copy(result, episodes)
	return result, nil
}

// GetRecent returns the most recent N episodes for a session.
func (s *Store) GetRecent(ctx context.Context, sessionKey string, limit int) ([]*Episode, error) {
	s.mu.RLock()
	defer s.mu.RUnlock()

	episodes, ok := s.episodes[sessionKey]
	if !ok {
		return []*Episode{}, nil
	}

	if limit <= 0 {
		limit = 10
	}

	start := len(episodes) - limit
	if start < 0 {
		start = 0
	}

	result := make([]*Episode, len(episodes)-start)
	copy(result, episodes[start:])
	return result, nil
}

// Search performs a simple case-insensitive text search across all episodes.
func (s *Store) Search(ctx context.Context, query string, limit int) ([]*Episode, error) {
	if limit <= 0 {
		limit = 20
	}

	s.mu.RLock()
	defer s.mu.RUnlock()

	queryLower := strings.ToLower(query)
	var results []*Episode

	for _, episodes := range s.episodes {
		for _, ep := range episodes {
			if strings.Contains(strings.ToLower(ep.Content), queryLower) {
				results = append(results, ep)
				if len(results) >= limit {
					return results, nil
				}
			}
		}
	}

	// Also search tags
	if len(results) < limit {
		for _, episodes := range s.episodes {
			for _, ep := range episodes {
				// Skip if already in results
				found := false
				for _, r := range results {
					if r.ID == ep.ID {
						found = true
						break
					}
				}
				if found {
					continue
				}

				for _, tag := range ep.Tags {
					if strings.Contains(strings.ToLower(tag), queryLower) {
						results = append(results, ep)
						break
					}
				}
				if len(results) >= limit {
					return results, nil
				}
			}
		}
	}

	return results, nil
}

// DeleteSession removes all episodes for a given session and deletes the JSONL file.
func (s *Store) DeleteSession(ctx context.Context, sessionKey string) error {
	s.mu.Lock()
	defer s.mu.Unlock()

	delete(s.episodes, sessionKey)

	path := filepath.Join(s.path, sessionKey+".jsonl")
	if err := os.Remove(path); err != nil && !os.IsNotExist(err) {
		return fmt.Errorf("episodic: delete session file: %w", err)
	}

	return nil
}

// Cleanup removes episodes older than the specified duration and returns the count removed.
func (s *Store) Cleanup(ctx context.Context, olderThan time.Duration) (int, error) {
	s.mu.Lock()
	defer s.mu.Unlock()

	cutoff := time.Now().UTC().Add(-olderThan)
	totalRemoved := 0

	for sessionKey, episodes := range s.episodes {
		var remaining []*Episode
		for _, ep := range episodes {
			if ep.Timestamp.After(cutoff) {
				remaining = append(remaining, ep)
			}
		}

		removed := len(episodes) - len(remaining)
		if removed > 0 {
			totalRemoved += removed
			if len(remaining) == 0 {
				delete(s.episodes, sessionKey)
				path := filepath.Join(s.path, sessionKey+".jsonl")
				os.Remove(path)
			} else {
				s.episodes[sessionKey] = remaining
				if err := s.rewriteSessionJSONL(sessionKey); err != nil {
					return totalRemoved, err
				}
			}
		}
	}

	return totalRemoved, nil
}

// Close flushes pending writes (in-memory data is always persisted on StoreEpisode).
func (s *Store) Close() error {
	return nil
}

// SessionCount returns the number of sessions stored.
func (s *Store) SessionCount() int {
	s.mu.RLock()
	defer s.mu.RUnlock()
	return len(s.episodes)
}

// EpisodeCount returns the total number of episodes across all sessions.
func (s *Store) EpisodeCount() int {
	s.mu.RLock()
	defer s.mu.RUnlock()

	total := 0
	for _, episodes := range s.episodes {
		total += len(episodes)
	}
	return total
}
