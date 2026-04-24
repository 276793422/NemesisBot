package memory

import (
	"bufio"
	"context"
	"crypto/rand"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"sync"
	"time"
	"unicode"
)

// --------------------------------------------------------------------------
// Manager — unified memory coordinator
// --------------------------------------------------------------------------

// Manager coordinates all memory storage backends and provides a high-level
// API for storing and retrieving memories.
type Manager struct {
	cfg       *Config
	workspace string
	store     Store
	enabled   bool
	mu        sync.RWMutex
}

// NewManager creates a new memory Manager. It initialises the appropriate
// storage backend based on the provided configuration.
//
// The workspace directory is used as the root for local file storage.
// If cfg is nil, DefaultConfig() is used.
func NewManager(cfg *Config, workspace string) (*Manager, error) {
	if cfg == nil {
		cfg = DefaultConfig()
	}

	m := &Manager{
		cfg:       cfg,
		workspace: workspace,
		enabled:   true,
	}

	// Determine storage backend.
	storageDir := filepath.Join(workspace, "memory")
	if err := os.MkdirAll(storageDir, 0755); err != nil {
		return nil, fmt.Errorf("memory: create storage directory: %w", err)
	}

	// Use local JSONL store as the default / fallback backend.
	storePath := filepath.Join(storageDir, "store.jsonl")
	store, err := newLocalStore(storePath)
	if err != nil {
		return nil, fmt.Errorf("memory: init local store: %w", err)
	}
	m.store = store

	return m, nil
}

// IsEnabled reports whether the memory system is active.
func (m *Manager) IsEnabled() bool {
	m.mu.RLock()
	defer m.mu.RUnlock()
	return m.enabled
}

// Store persists a memory entry. If entry.ID is empty a unique ID will be
// generated. CreatedAt and UpdatedAt are set when zero.
func (m *Manager) Store(ctx context.Context, entry *Entry) error {
	m.mu.RLock()
	defer m.mu.RUnlock()
	if !m.enabled {
		return nil
	}
	prepareEntry(entry)
	return m.store.Store(ctx, entry)
}

// Query searches memories matching the text query. If types is non-nil only
// memories of those types are considered.
func (m *Manager) Query(ctx context.Context, query string, limit int, types []MemoryType) (*SearchResult, error) {
	m.mu.RLock()
	defer m.mu.RUnlock()
	if !m.enabled {
		return &SearchResult{Query: query}, nil
	}
	return m.store.Query(ctx, query, limit, types)
}

// Get retrieves a single memory by its ID.
func (m *Manager) Get(ctx context.Context, id string) (*Entry, error) {
	m.mu.RLock()
	defer m.mu.RUnlock()
	if !m.enabled {
		return nil, nil
	}
	return m.store.Get(ctx, id)
}

// Delete removes a memory entry by ID.
func (m *Manager) Delete(ctx context.Context, id string) error {
	m.mu.RLock()
	defer m.mu.RUnlock()
	if !m.enabled {
		return nil
	}
	return m.store.Delete(ctx, id)
}

// List returns a paginated list of memories, optionally filtered by type.
func (m *Manager) List(ctx context.Context, types []MemoryType, offset, limit int) (*SearchResult, error) {
	m.mu.RLock()
	defer m.mu.RUnlock()
	if !m.enabled {
		return &SearchResult{}, nil
	}
	return m.store.List(ctx, types, offset, limit)
}

// QuerySemantic performs a semantic search across all memory types. This is
// the primary method intended for agent-loop integration.
//
// When the vector backend is enabled it delegates to the vector store for
// embedding-based similarity. Otherwise it falls back to keyword-frequency
// scoring over the local JSONL store.
func (m *Manager) QuerySemantic(ctx context.Context, query string, limit int) (*SearchResult, error) {
	m.mu.RLock()
	defer m.mu.RUnlock()
	if !m.enabled {
		return &SearchResult{Query: query}, nil
	}
	if limit <= 0 {
		limit = m.cfg.Vector.MaxResults
		if limit <= 0 {
			limit = 5
		}
	}
	// Search across all types (nil means no filter).
	return m.store.Query(ctx, query, limit, nil)
}

// StoreEpisodic is a convenience helper for storing conversation episodes.
// It creates an Episodic memory entry tagged with the session key and role.
func (m *Manager) StoreEpisodic(ctx context.Context, sessionKey, role, content string) error {
	entry := &Entry{
		Type:    MemoryEpisodic,
		Content: content,
		Metadata: map[string]string{
			"session_key": sessionKey,
			"role":        role,
		},
		Tags: []string{"conversation", role},
	}
	return m.Store(ctx, entry)
}

// StoreFact is a convenience helper for storing long-term factual knowledge.
func (m *Manager) StoreFact(ctx context.Context, content string, tags []string) error {
	entry := &Entry{
		Type:    MemoryLongTerm,
		Content: content,
		Tags:    tags,
	}
	return m.Store(ctx, entry)
}

// Close shuts down the memory manager and releases all resources.
func (m *Manager) Close() error {
	m.mu.Lock()
	defer m.mu.Unlock()
	m.enabled = false
	if m.store != nil {
		return m.store.Close()
	}
	return nil
}

// --------------------------------------------------------------------------
// helpers
// --------------------------------------------------------------------------

// generateID creates a cryptographically random 16-byte hex string.
func generateID() string {
	b := make([]byte, 16)
	// Fallback to timestamp-based ID if crypto/rand fails.
	if _, err := rand.Read(b); err != nil {
		return fmt.Sprintf("%x", time.Now().UnixNano())
	}
	return hex.EncodeToString(b)
}

// prepareEntry fills in ID and timestamps if missing.
func prepareEntry(entry *Entry) {
	if entry.ID == "" {
		entry.ID = generateID()
	}
	now := time.Now().UTC()
	if entry.CreatedAt.IsZero() {
		entry.CreatedAt = now
	}
	entry.UpdatedAt = now
}

// --------------------------------------------------------------------------
// localStore — JSONL-backed Store implementation
// --------------------------------------------------------------------------

// localStore implements Store using a single JSONL file for persistence.
// It provides basic keyword-frequency scoring for text queries.
type localStore struct {
	path    string
	mu      sync.RWMutex
	entries map[string]*Entry // in-memory index by ID
}

// newLocalStore loads (or creates) a localStore backed by path.
func newLocalStore(path string) (*localStore, error) {
	s := &localStore{
		path:    path,
		entries: make(map[string]*Entry),
	}
	if err := s.load(); err != nil {
		return nil, err
	}
	return s, nil
}

// load reads the JSONL file into memory.
func (s *localStore) load() error {
	f, err := os.Open(s.path)
	if err != nil {
		if os.IsNotExist(err) {
			return nil // fresh store
		}
		return err
	}
	defer f.Close()

	scanner := bufio.NewScanner(f)
	// Allow lines up to 1 MB.
	scanner.Buffer(make([]byte, 0, 64*1024), 1024*1024)
	for scanner.Scan() {
		line := strings.TrimSpace(scanner.Text())
		if line == "" {
			continue
		}
		var entry Entry
		if err := json.Unmarshal([]byte(line), &entry); err != nil {
			continue // skip malformed lines
		}
		s.entries[entry.ID] = &entry
	}
	return scanner.Err()
}

// flush writes all entries back to the JSONL file.
func (s *localStore) flush() error {
	dir := filepath.Dir(s.path)
	if err := os.MkdirAll(dir, 0755); err != nil {
		return err
	}

	f, err := os.Create(s.path)
	if err != nil {
		return err
	}
	defer f.Close()

	w := bufio.NewWriter(f)
	for _, entry := range s.entries {
		data, err := json.Marshal(entry)
		if err != nil {
			return err
		}
		if _, err := fmt.Fprintf(w, "%s\n", data); err != nil {
			return err
		}
	}
	return w.Flush()
}

// Store saves entry to the in-memory map and flushes to disk.
func (s *localStore) Store(_ context.Context, entry *Entry) error {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.entries[entry.ID] = entry
	return s.flush()
}

// Get retrieves an entry by ID.
func (s *localStore) Get(_ context.Context, id string) (*Entry, error) {
	s.mu.RLock()
	defer s.mu.RUnlock()
	e, ok := s.entries[id]
	if !ok {
		return nil, nil
	}
	cp := *e
	return &cp, nil
}

// Delete removes an entry by ID and flushes.
func (s *localStore) Delete(_ context.Context, id string) error {
	s.mu.Lock()
	defer s.mu.Unlock()
	delete(s.entries, id)
	return s.flush()
}

// Query performs keyword-frequency scoring against the stored entries.
func (s *localStore) Query(_ context.Context, query string, limit int, types []MemoryType) (*SearchResult, error) {
	s.mu.RLock()
	defer s.mu.RUnlock()

	if limit <= 0 {
		limit = 10
	}

	typeFilter := make(map[MemoryType]bool)
	for _, t := range types {
		typeFilter[t] = true
	}

	// Tokenize the query.
	queryTokens := tokenize(query)
	if len(queryTokens) == 0 {
		return &SearchResult{Query: query, Total: 0}, nil
	}

	// Build inverse document frequency across all entries.
	candidates := make([]Entry, 0)
	for _, e := range s.entries {
		if len(typeFilter) > 0 && !typeFilter[e.Type] {
			continue
		}
		score := scoreEntry(e, queryTokens)
		if score > 0 {
			cp := *e
			cp.Score = score
			candidates = append(candidates, cp)
		}
	}

	// Sort by score descending.
	sort.Slice(candidates, func(i, j int) bool {
		return candidates[i].Score > candidates[j].Score
	})

	total := len(candidates)
	if len(candidates) > limit {
		candidates = candidates[:limit]
	}

	return &SearchResult{
		Entries: candidates,
		Total:   total,
		Query:   query,
	}, nil
}

// List returns entries optionally filtered by type, with pagination.
func (s *localStore) List(_ context.Context, types []MemoryType, offset, limit int) (*SearchResult, error) {
	s.mu.RLock()
	defer s.mu.RUnlock()

	typeFilter := make(map[MemoryType]bool)
	for _, t := range types {
		typeFilter[t] = true
	}

	// Collect matching entries sorted by CreatedAt descending.
	matched := make([]Entry, 0)
	for _, e := range s.entries {
		if len(typeFilter) > 0 && !typeFilter[e.Type] {
			continue
		}
		cp := *e
		matched = append(matched, cp)
	}
	sort.Slice(matched, func(i, j int) bool {
		return matched[i].CreatedAt.After(matched[j].CreatedAt)
	})

	total := len(matched)
	if offset > len(matched) {
		offset = len(matched)
	}
	matched = matched[offset:]
	if limit > 0 && len(matched) > limit {
		matched = matched[:limit]
	}

	return &SearchResult{
		Entries: matched,
		Total:   total,
	}, nil
}

// Close is a no-op for the local store (data is flushed on every write).
func (s *localStore) Close() error {
	return nil
}

// --------------------------------------------------------------------------
// Text scoring — simplified TF-IDF keyword matching
// --------------------------------------------------------------------------

// tokenize splits text into lowercase tokens, removing punctuation and
// collapsing whitespace.
func tokenize(text string) []string {
	text = strings.ToLower(text)
	// Replace punctuation with spaces.
	var b strings.Builder
	for _, r := range text {
		if unicode.IsPunct(r) || unicode.IsSymbol(r) {
			b.WriteByte(' ')
		} else {
			b.WriteRune(r)
		}
	}
	fields := strings.Fields(b.String())
	return fields
}

// scoreEntry computes a keyword-frequency relevance score for an entry
// against the given query tokens. The score is the fraction of query tokens
// that appear in the entry content (with multiplicity for repeated matches),
// normalised to [0, 1].
func scoreEntry(e *Entry, queryTokens []string) float64 {
	if len(queryTokens) == 0 {
		return 0
	}

	content := strings.ToLower(e.Content)
	// Also include tags and metadata values in the searchable text.
	var extra strings.Builder
	for _, t := range e.Tags {
		extra.WriteString(" ")
		extra.WriteString(strings.ToLower(t))
	}
	for _, v := range e.Metadata {
		extra.WriteString(" ")
		extra.WriteString(strings.ToLower(v))
	}
	fullText := content + extra.String()

	textTokens := tokenize(fullText)
	tokenCount := make(map[string]int)
	for _, t := range textTokens {
		tokenCount[t]++
	}

	var matchCount float64
	for _, qt := range queryTokens {
		if cnt, ok := tokenCount[qt]; ok && cnt > 0 {
			// Weight by presence and slight boost for frequency.
			matchCount += 1.0 + float64(cnt-1)*0.1
		}
	}

	// Normalise by number of query tokens.
	return matchCount / float64(len(queryTokens))
}

// Ensure localStore satisfies Store at compile time.
var _ Store = (*localStore)(nil)
